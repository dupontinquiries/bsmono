# SnapClone — TODO

Tactical list. For strategic direction see [ROADMAP.md](ROADMAP.md);
for design philosophy see [VISION.md](VISION.md).

Status legend:
- 🔥 Blocker — actively broken in production builds
- ⚠️  Priority — needs fixing soon
- 🟡 Normal
- 🟢 Polish / nice-to-have
- 🧪 Needs investigation before we know what to do
- 🚫 Decided NOT to do (kept for posterity / future revisit)

---

## 🔥 Active blockers

> **▶ NEXT DEBUG SESSION — start here (V32.34+ unified-nav build).**
> All of the following was written + brace-checked + logic-reviewed but is
> UNTESTED on device. Verify in this order; each gates the ones below it.
>
> 1. **CLEAN EXIT (highest priority).** Does the app exit WITHOUT the
>    `malloc_consolidate` / QStyleSheetStyleCaches abort at shutdown? The fix
>    (main.cpp, after exec(): `qApp->setStyleSheet("")` + window clear +
>    processEvents) is the foundation everything else sits on. If it STILL
>    aborts at teardown → stop testing UI, go to the fallback: move the
>    SettingsPage theme-carousel off per-widget setStyleSheet to objectName
>    selectors in the global sheet (that subtree was the one in the backtrace).
>    `coredumpctl gdb` → `bt` again to confirm it's still the same frame.
> 2. **MID-SESSION Wayfinder crash STILL gone?** The touch-tap-vs-swipe guard
>    (WayfinderPage::event, kTapMaxTravelPx=16) fixed it once ("crash avoided").
>    Confirm it survived the later changes. Repro was: swipe LEFT landing on
>    Wayfinder.
> 3. **SC_UNIFIED_NAV=1 visual sanity.** Bottom bar should now: render the
>    current page's sub-tabs as segments; show count pills (unread/online);
>    show page-position dots up top; center the name on Camera/Wayfinder;
>    collapse labels on a narrow window. Top in-page tab strips should be GONE.
> 4. **GESTURE FEEL (needs your hand, can't be reasoned).** Vertical
>    scroll-to-limit-and-pull → sub-tab advance; horizontal swipe → sub-tab
>    cycle, hard push → page change; the friction + barrier glow + the bar's
>    in-flight underline. Tune the SEED constants:
>      - SwipeNavigator: kDetentArmWindowMs=1000, kVDetentRawThreshold=220,
>        kHDetentBreakPx=90, kHDetentBreakPx friction sqrt curve.
>      - UnifiedNavBar underline alpha ramp 0.55→0.90.
> 5. **Watch for cramped tabs over a long session** — the clearSegments stretch
>    leak is fixed, but confirm tabs DON'T shrink after many page flips.
>
> If something's broken and it's in the unified-nav code, the fastest isolation
> is `SC_UNIFIED_NAV=0` (everything new is behind that flag and inert when off)
> — if the problem persists with the flag off, it's pre-existing, not this work.
>
> **INERTNESS VERIFIED (by inspection, this build):** with SC_UNIFIED_NAV unset,
> MainWindow takes the `else` branch (old TabBar mounted, m_unifiedBar stays
> nullptr), the badge-wiring connects never run (they're inside the flag-on
> branch), the toast positioning null-guards m_unifiedBar, and all 3 page
> top-strip hides are gated `unifiedNavEnabled() && m_filterTabs` so the strips
> stay visible. Every SwipeNavigator gate short-circuits on unifiedNavEnabled()
> first, and UnifiedNavBar::paintEvent never runs because the bar isn't
> constructed. So flag-off == the pre-existing V32.33 UI, byte-for-byte. This is
> a TRUSTWORTHY known-good fallback to debug against.

*Investigated & patched but pending real-device confirmation:*

- [ ] **Wire-format tier byte for tiered delivery — V32.17 PARTIAL.**
  V32.4 scaffolded TieredDeliveryPolicy + EncodeTier on snap data;
  v5 wire format now transmits the tier byte (additive after v4's
  videoDurationMs field). Snap struct + storage schema (v12) both
  carry the field. **Still TODO:**
    - Receiver stores multiple versions keyed by `(snapId, tier)` and
      lazy-upgrades (currently the latest-arrival overwrites, which is
      good enough for first deployment but loses the "thumbnail-then-
      better" UX we want)
    - ANNOUNCE/HAVE/NEED extended to include tier so peers can request
      a specific tier they don't have yet
    - UI side: Inbox displays current-best tier with a "loading better
      version" indicator when an upgrade is in flight
    - TieredDeliveryPolicy needs to actually call the encoder
      multiple times — currently single-shot. Encode-once-send-all-tiers
      schedule TBD.
- [ ] Mobile crash on sending a snap — root cause was synchronous
  `snapCaptured` re-emit from inside `ContactPickerDialog::accept()`,
  which re-entered the camera-hide event handler while the picker was
  mid-animation. Fix: deferred re-emit via
  `QMetaObject::invokeMethod(... QueuedConnection)`. Needs phone test.
  [V28]
- [ ] Desktop v4l2 buffer pool activation failure on FaceTime HD.
  Root cause: USB bandwidth not released until `QCamera` is destroyed,
  not just `stop()`-ed. Fix: full pipeline teardown in
  `QtCameraBackend::stop()` + 1500ms fixed wait. Needs clean rebuild
  + retest.
- [ ] Mobile transcode freeze at 0% — three contributing causes
  identified and patched: hardcoded `svtav1enc` (now probes fallbacks
  vp9/vp8), hardcoded audio branch (now detects video-only inputs),
  no progress signal until output bytes flush (now combines file-size
  estimate with time-based 1%/3s creep). Needs phone test.

---

## ⚠️ Priority — known issues with planned fixes

### Single-instance enforcement + multi-account switching (design brief, V32.34+)

**Problem.** There is currently NO single-instance guard. Nothing stops a
user from launching SnapClone N times. Each launch runs the full
`controller.initialize()` against SHARED single-writer resources, and the
damage is correctness, not just battery:

  1. **SQLite contention/corruption.** Every instance opens the same
     `AppDataLocation/db/snapclone.sqlite`. Concurrent writers → `database
     is locked` stalls at best, genuine corruption at worst (esp. WAL +
     a crash).
  2. **DHT identity incoherence.** Both instances announce the SAME sc-id
     with DIFFERENT ephemeral connection details (ports, session keys).
     Peers polling our mailbox / resolving our address get conflicting
     "reach me here" records racing each other. A sender may connect to
     instance A's port while instance B holds the session key they expect.
     This actively BREAKS reachability — not merely wasteful.
  3. **Port binding.** The WS/UDP listeners: the second instance either
     fails to bind (silent degrade) or grabs a different port, compounding
     #2.
  4. **DHT + battery amplification.** N nodes bootstrap, announce,
     re-announce, each runs its own DhtResourceManager. N× DHT traffic and
     battery — the exact "open it 100 times and blow up networking +
     battery" failure the user flagged.

So single-instance is justified by CORRECTNESS (DB integrity + DHT identity
coherence), with the battery/traffic win as a bonus. There is no good
reason to allow two live processes for the same profile.

**Design — single-instance PER PROFILE (not merely per machine):**

  - On startup, before `controller.initialize()`, acquire a per-profile
    lock. `QLockFile` at `AppDataLocation/<profile>/.instance.lock` is the
    portable mechanism (handles stale locks from crashes via PID liveness
    check — QLockFile does this automatically with `setStaleLockTime`).
  - Pair it with a `QLocalServer`/`QLocalSocket` named by the profile so a
    second launch can hand off "raise/focus the existing window" (and any
    file/URL it was launched with) to the running instance, then exit 0.
    On Plasma Mobile "raise the window" may be a no-op, but the second
    process MUST still exit rather than run a second stack.
  - If the lock is held and the local socket handshake succeeds → forward
    intent, exit. If the lock is held but the socket is dead (crashed
    holder) → QLockFile's stale detection reclaims it; proceed.

**Multi-account is the natural complement (user's insight).** Instead of
"launch again for a second account" (which would reintroduce all of the
above), multi-account = SWITCH PROFILES INSIDE THE ONE PROCESS:

  - A "profile" = an isolated data root: its own identity.txt, its own
    SQLite db, its own media dir, its own QSettings group. Today
    `AppDataLocation` is hardcoded in ≥4 places (Identity.cpp,
    SnapRouter.cpp, AppController.cpp db path, AppController_misc.cpp) —
    step 1 is to thread a `profileRoot` through these instead of calling
    `QStandardPaths::AppDataLocation` directly. Default profile root =
    `AppDataLocation/profiles/default/` (migrate existing data into it on
    first run of the new scheme).
  - Switching account: tear DOWN the live stack for profile A — withdraw
    DHT announce, set DhtMode::Off, close the DB, zeroize session keys
    (composes with S-8) — THEN bring up profile B's stack. Only ever ONE
    DHT identity live at a time, so identity coherence is preserved and
    battery stays flat regardless of how many accounts exist.
  - UI: a profile switcher in Settings (or a long-press on the identity
    chip). Lists profiles, shows the active one, "+ Add account" creates a
    new profile root + identity, "Switch" does the teardown/bringup dance.
    A switch is heavier than a page change (re-init networking) so show a
    brief "Switching account…" state.

**Sequencing:**
  1. [ ] **Single-instance guard FIRST** (self-contained, immediate
     correctness+battery win, no UI). QLockFile + QLocalServer in main()
     before initialize(). This is the down-payment below.
  2. [ ] **Parameterize the data root** — thread `profileRoot` through
     Identity / StorageManager / SnapRouter / AppController instead of
     hardcoded AppDataLocation. Migrate existing data into
     `profiles/default/`. No behavior change yet (one profile).
  3. [ ] **Profile teardown/bringup** in AppController (a `switchProfile()`
     that cleanly stops + restarts the stack). Reuses the DhtMode::Off
     path + DB close + key zeroize.
  4. [ ] **Profile switcher UI** + "Add account".
  5. [ ] Lock is per-profile, so two DIFFERENT profiles COULD run as two
     processes if we ever wanted that — but default to switch-in-place.
     Decide later whether to allow it; the per-profile lock makes it a
     policy toggle, not an architecture change.

**Open questions:**
  - On pmOS, is the app launched via a `.desktop` that could pass a
    "raise" action, or do we rely purely on the QLocalServer handoff?
  - Should `--profile <name>` be a CLI flag (useful for testing two
    identities on one machine via two data roots)? Cheap to add once the
    root is parameterized; keep it dev-only / undocumented initially.
  - Identity migration: existing installs have data directly in
    AppDataLocation. The default-profile migration must be a one-time,
    idempotent move guarded by a marker file, and must NOT run if a
    profiles/ dir already exists.


- [x] ~~Stale comment near `peerConnected` listener in `MainWindow.cpp`
  ("Outbound snap delivered" → it's actually tracking online peers).
  Cosmetic, low-risk.~~ **Already resolved** — checked in V32.26
  scan, the comment at the `peerConnected` listener now correctly
  describes "Track which peers are currently online." Some earlier
  pass cleaned it up; closing the bookkeeping.
- [x] ~~`writeMediaFile` saves video media as `.mp4` regardless of actual
  container.~~ **Fixed V30** — detects container by magic bytes (EBML
  header → `.webm`, `ftyp` → `.mp4`, JPEG/PNG → image extensions).
- [ ] Where do bogus loopback peer addresses originate? Three layers
  of guards now block them, but the upstream injection point is
  unknown. Watch logs for `connectToPeer: refusing` warnings — they
  name the calling path.

---

## 🔋 Battery management, DHT sub-management & sleep states (V32.34+)

Mobile testing shows battery life is the app's biggest weak point. The
infrastructure for power-aware behavior was largely written long ago
(`PowerManager`, `DhtResourceManager`) but never instantiated — it ran
on optimistic defaults, so none of the conservation logic actually
fired. V32.34+ turns the stack ON and adds camera sleep-on-screen-off.
This section tracks what's done and what's left.

### Done (V32.34+)
- [x] **Instantiate + attach PowerManager** in `AppController::initialize()`.
  Three-tier integration now live: PowerDevil (`org.kde.Solid.PowerManagement`
  + `org.kde.screensaver`), upower (`org.freedesktop.UPower`), `/sys` +
  systemd-logind. Emits screen/battery/thermal/profile signals.
- [x] **Instantiate + wire DhtResourceManager.** 4-tier DHT mode
  (Full / Maintenance / Minimal / Off) driven by battery %, charging,
  screen on/off, thermal state, power profile. Active P2P sessions pin
  it to Full via a peerConnected/peerDisconnected live-session counter.
- [x] **Camera sleeps on screen-off.** CameraWidget subscribes to
  `PowerManager::screenActiveChanged`; on screen-off it fully STOPS the
  GStreamer pipeline + camera-bridge (sensor/ISP/preview-encoder
  power-down) rather than merely dropping frames, and stops the 1Hz
  landmark scorer + scene-change focus tick. Auto-restarts on screen-on
  IFF we were the one who stopped it and the camera page is still visible.
- [x] **Per-component app-state gating** (earlier in V32.34+): AmbientBlobs,
  WayfinderPage pulse, AmbientCameraDriver derive all stop their timers
  when the app backgrounds. DhtResourceManager uptime counter dropped
  from 1Hz to a 60s VeryCoarseTimer.

### Battery management — remaining
- [ ] **Surface power state in the UI.** SettingsPage currently polls
  `/sys` directly as a stand-in (pre-PowerManager-wiring). Replace that
  poll with live `PowerManager` signal subscriptions now that it's
  instantiated and reachable via `AppController::powerManager()`. Show
  battery %, charging, thermal state, DHT mode + reason, and a "power
  saver active" banner when `isBatterySaver()`.
- [ ] **User-facing battery-saver toggle.** A manual override that forces
  conservative behavior regardless of charge level (some users want max
  battery even at 80%). Persist to QSettings; feed into
  DhtResourceManager (force ≤ Minimal) and the camera (lower viewfinder
  resolution, longer focus debounce).
- [ ] **Thermal-aware camera resolution.** PowerManager already exposes
  `thermalState()`. When Warm/Hot, drop the viewfinder capture resolution
  (e.g. 1280×720 → 854×480) and disable the landmark scorer. Wire
  `thermalStateChanged` into CameraPolicy at start() time.
- [ ] **Suspend-inhibit only during active transfer.** PowerManager has
  `onSessionStarted/Ended` gating the logind/KDE inhibit. Confirm these
  are actually called from the snap send/receive paths (currently wired
  to peer connect — but a *connected idle* peer shouldn't block suspend;
  only an in-flight transfer should). Tighten to transfer-scoped.
- [ ] **Measure.** Add a debug HUD (behind a UiDebugFlag) showing wakeups/
  min, current DHT mode, screen state, and camera pipeline state, so the
  next on-device session can quantify the win rather than guessing.

### DHT sub-management — remaining
- [ ] **Verify the mode actually changes on device.** DhtResourceManager
  logs `DHT mode → X (reason)` on every transition. Capture a screen-off
  trace and confirm: screen-on+battery → Full, screen-off+battery →
  Maintenance/Minimal, low battery → Off. The mode→reannounce-interval
  mapping needs real-world validation.
- [ ] **Re-announce interval per mode is currently a guess.** Tune
  `reannounceIntervalMs()` against actual DHT reachability: too sparse
  and peers can't find us after we background; too frequent and we burn
  radio. Needs a "did a peer reach me while backgrounded" metric.
- [ ] **Minimal mode semantics.** Today Minimal still runs the DHT node
  (just announces rarely). Consider whether Minimal should instead keep
  the routing table warm but stop announcing entirely — cheaper, and we
  re-announce instantly on screen-on. Decision needs the reachability
  metric above.

- [ ] **🔋 Limit DHT PORT traffic when screen-off (the real radio cost) —
  ANALYSIS (V32.34+).** Question raised: screen-off still shows steady
  DHT port activity; can we cut it, and does it need tighter OpenDHT
  integration / a fork? Findings:
  * **What the mode system controls today is NOT the dominant traffic.**
    `DhtResourceManager` has exactly two levers: `setReannounceInterval()`
    (how often WE re-publish our value — stretched to 8h in Minimal) and
    full `stop()`. Stretching our re-announce does cut OUR periodic
    announces, but the OpenDHT NODE keeps doing its own background work
    regardless: routing-table bucket refreshes, responding to other peers'
    get/put queries, neighbor keepalives, bootstrap liveness. THAT is the
    port chatter seen with screen off — and the re-announce knob doesn't
    touch it. So between "announce rarely" and "fully off" there's
    currently nothing.
  * **Current integration is bare.** `JamiAdapter_dht_impl.cpp` starts the
    node with `dht_runner_run(runner, 0)` — any port, ALL defaults, no
    config struct. So the node runs full default maintenance.
  * **Options, least→most invasive (NO fork needed for any of these — they
    use opendht's public API/config; fork only if we wanted to change
    opendht's internal refresh cadence itself, which we don't):**
    1. **Aggressive Minimal = Off + fast wake (cheapest, recommended
       first).** Treat screen-off as: stop the node entirely, but keep the
       routing table / our announced values cached in memory, and on
       screen-on call start()+re-announce immediately (we already reconnect
       in ~1 announce). Net: ZERO port traffic while pocketed, ~1–2s
       reachability lag on wake. Pairs with the existing "Coordinate with
       mailbox checks" item — bring DHT up briefly for the 5-min mailbox
       poll, then drop. This is mostly DhtResourceManager policy + the
       mailbox-timer gate; no opendht API change. Tradeoff: we're
       unreachable via DHT while screen-off EXCEPT during the brief mailbox
       windows — acceptable since inbound delivery already flows through
       the mailbox/connreq path, not a live DHT presence.
    2. **Proxy-client mode (moderate, opendht-supported, no fork).**
       OpenDHT can run as a lightweight PROXY CLIENT (talks to a DHT proxy
       server over HTTP and offloads the routing-table maintenance + push
       notifications to the proxy). This is exactly what Jami does for
       mobile battery life. Port chatter collapses to occasional HTTPS to
       one proxy instead of constant UDP to many peers. Requires: a proxy
       server URL (Jami runs public ones, or self-host), and switching the
       C-API startup to proxy config (`dht_runner_run_config` with a
       proxy_server set — need to confirm the exact field in our
       opendht_c.h version; the C API exposes it). Bigger lift: the adapter
       currently assumes direct-node semantics. But it's the "right" mobile
       answer and needs no fork.
    3. **Node-config tuning (minor, if the C API exposes it).** Some
       OpenDHT builds let you set the node to not maintain storage
       (`maintain_storage=false`) and reduce its server role — cuts the
       "serve other peers' queries" share. Check whether our opendht_c.h's
       run-config struct exposes these; if so it's a cheap partial win
       stackable with (1).
  * **RECOMMENDATION:** do (1) first — it's policy-only, no opendht API
    risk, and screen-off→zero-traffic is the biggest battery win.
    **IMPLEMENTED V32.34+ (opt-in, needs device validation):**
    `DhtResourceManager::setAggressiveScreenOff(bool)` — when on, screen-off
    on battery (no active session, not charging) makes computeTargetMode
    return Off instead of Minimal/Maintenance, fully silencing the node's
    port chatter. Placed AFTER the thermal/session/AC guards so it never
    kills an active call or a charging device. Gated behind
    `SC_DHT_PARK_SCREEN_OFF=1` (env) or QSettings `dht/parkScreenOff`,
    wired in AppController where the manager is built. Log reason added so a
    screen-off trace shows "screen off — DHT parked (aggressive), reconnect
    on wake". KNOWN TRADEOFF while parked: the 5-min mailbox poll calls
    checkConnectionRequests on a stopped runner — safe (opendht get just
    returns nothing, no crash) but non-functional, so inbound connection
    requests aren't seen until screen-on. The "bring DHT up briefly for the
    mailbox poll" enhancement (separate TODO item below) closes that gap
    and should land before this becomes default. DEVICE-VALIDATE: enable
    the flag, screen-off trace should show mode→Off with the parked reason
    and the port traffic should drop to zero; screen-on should reconnect +
    re-announce within ~1–2s.
    Evaluate (2) proxy-client later if we want true screen-off reachability
    without the radio cost — that's the only one that needs real opendht-
    integration work, and even it doesn't need a fork. Confirm exact
    `dht_runner_*` config fields against the on-device opendht_c.h before
    coding (header isn't in this build env).
- [ ] **Coordinate with mailbox checks.** The 5-minute DHT mailbox poll
  (`AppController` mailboxTimer) runs independent of DHT mode. When the
  mode is Off, the mailbox check will fail (no DHT node). Either gate the
  mailbox timer on mode, or have it briefly bring DHT up to do the check
  then drop back — the latter is better for "receive while backgrounded."

### Sleep states — design + remaining
A coherent state machine the whole app shares, rather than each component
reacting to screen/app signals independently.

**BUILT (V32.34+).** `core/SleepState.{hpp,cpp}` exists:
`SleepStateMachine` consumes PowerManager (screen/charge) + AppContext
(foreground/idle) and emits a single `sleepStateChanged(SleepState)`.
Instantiated in `AppController::initialize()` right after PowerManager +
AppContext; reachable via `AppController::sleepStateMachine()`. Includes
the transition debounce (immediate toward more-active, kDebounceMs=2.5s
delay toward conservation, so a notification peek doesn't thrash the
camera pipeline) and the Active→ForegroundIdle idle timer
(kIdleAfterMs=30s). Decision table verified against an 11-case truth
table (screen off dominates; charging overrides background/screen-off
toward less conservation but NOT foreground-active). Convenience
predicates `allowsCosmeticWork()` / `allowsCamera()`.

States:
- [x] **`Active`** — app foreground, screen on, recent interaction.
- [x] **`ForegroundIdle`** — foreground, screen on, idle ≥30s.
- [x] **`Background`** — app not foreground.
- [x] **`ScreenOff`** — device screen off (deep pocket-sleep).
- [x] **`Charging`** — plugged in + idle/backgrounded/asleep.

**Remaining — migrate the existing ad-hoc gating onto this signal:**
- [ ] AmbientBlobs, WayfinderPage pulse, AmbientCameraDriver currently
  each subscribe to `QGuiApplication::applicationStateChanged` directly.
  Fold each into a `sleepStateChanged` handler (stop on Background /
  ScreenOff; full rate only on Active; reduced on ForegroundIdle). These
  components need a controller/sleep-machine pointer threaded in first —
  they don't have one today, which is why the migration is deferred.
- [ ] CameraWidget already wires `PowerManager::screenActiveChanged`
  directly (prior session). Could migrate to `allowsCamera()` for a
  cleaner single gate, but the direct wiring works — low priority.
- [ ] DhtResourceManager has its own multi-signal `evaluate()`. Leave as
  is OR have it consult SleepState for the coarse decision and keep its
  fine battery-threshold logic. Decision: keep separate for now — DHT
  needs finer battery granularity than the 5-state machine exposes.
- [ ] Surface the current SleepState in the debug HUD (behind a
  UiDebugFlag) so the next on-device session can confirm transitions
  fire correctly (the AppController logs `SleepState → X` on every
  change already; grep `snapclone.sleepstate` in the journal).

---

## 🟡 Normal — things to do when there's time

### Storage / DB
- [x] ~~Schema migration on contacts table to add `last_connected_at`
  field for staleness-based eviction~~ **Done V32.27.** Migration 13
  adds `users.last_connected_at` (INTEGER ms-since-epoch, default 0).
  Schema-repair list also extended for the upgrade-from-old-build
  case. `StorageManager::touchLastConnected(UserId)` writes the
  current timestamp; AppController calls it on both outbound and
  inbound `peerConnected` paths. Distinct from `last_seen_at` which
  fires on any presence observation including bare gossip beacons —
  `last_connected_at` only fires after full session completion.
  Consumers (e.g. an eviction policy or "haven't talked to X in
  N days" notice) are pending; the data is being recorded now so
  it's ready when someone wants it.
- [x] ~~Storage migration runner doesn't currently rollback on failure
  (`src/storage/StorageManager.cpp` applyMigrations). Wrap in
  transaction.~~ **Already resolved.** V32.26 scan of
  `applyMigration` shows it wraps each migration in
  `m_db.transaction()` and calls `m_db.rollback()` on per-statement
  failure (line 509) AND on commit failure (line 523). The
  `user_version` pragma is issued *outside* the transaction
  deliberately so it lands even on some SQLite builds that drop
  pragmas inside transactions. Closing the bookkeeping.
- [x] ~~DhtCache pruning — currently grows unbounded. Cap to ~256
  entries with LRU eviction.~~ **Done V32.27.** Per-peer cap of 15
  was already in place; now `cacheDhtResult` additionally enforces
  a global 256-row cap. Eviction order: high `fail_count` first
  (drop the unreachable entries), then oldest `cached_at`. The
  COUNT pre-check means the DELETE only fires when over the cap,
  so typical (small-cache) writes incur a single SELECT COUNT(*).

### Network
- [ ] **🧠 App state → FSM coupling.** *V32.21 ships V1: infrastructure
  built + first consumer wired. Remaining work below — file as
  follow-ups when convenient.* The FSM (`FSMValidator` in
  `src/defense/`, the per-peer Phase state machine in `UdpChannel.cpp`)
  currently sees only narrow protocol-level signals: which frame
  arrived, which phase the peer is in, did the timer fire. It does
  NOT see the broader app state that should inform its decisions:
  whether the user is actively using the app, whether the screen is
  on, whether we just changed WiFi networks, whether the camera is
  open vs the inbox is open, what the system reports as connection
  type (cellular vs WiFi vs ethernet vs metered), whether power
  saver is on, whether the network has recently been unreachable.

  Without those signals the FSM picks aggressive defaults in
  scenarios where it should back off (e.g. spammy STUN retries on
  cellular with screen off → battery drain) and conservative
  defaults in scenarios where it should be aggressive (e.g. user
  just took a photo, definitely wants snappy delivery → push past
  the usual exponential backoff). Concretely we have evidence of
  both: the cafe-wifi logs from V32.15 showed minutes of relookups
  + spray-punches against a client-isolated AP that would never
  let direct work; conversely the morning-first-snap case has the
  user waiting on a cold reconnect that could be predicted.

  **Sketch of the work** (none of this is committed; treat as
  a design brief to refine when we pick it up):
    - Define an `AppContext` snapshot struct fed periodically into
      decision points. Fields: `screenOn`, `appActive` (foreground/
      visible), `currentPage` (camera/inbox/idle), `connectionType`
      (wifi/cellular/ethernet/none), `metered`, `powerSaver`,
      `recentSsidChangeMs`, `recentReachabilityFlapMs`,
      `userJustActed` (camera shutter / send button within N
      seconds).
    - A single `AppContextProvider` QObject in `core/` owns the
      struct, listens to QNetworkInformation + Qt::ApplicationState +
      CameraWidget visibility + power signals, and emits
      `contextChanged(AppContext)`.
    - Network-layer subscribers (NatTraversal punch loop, UdpChannel
      retransmit timer, AppController reconnect scheduler, DHT
      bootstrap retry) consult the latest context before deciding
      backoff / aggression. Concretely: punch volley count scales
      down when screenOff+metered; reconnect immediacy scales UP
      when userJustActed; LAN-isolation threshold (currently 2 from
      V32.15) becomes 1 when we know we're on cellular and LAN
      can't possibly work; DHT bootstrap retry interval stretches
      when reachability is flapping (no point bootstrapping while
      the network is half-broken).
    - The FSM's "this frame is misaligned with phase" logs (V32.7
      `FSMValidator`) become much more useful if they include
      the AppContext at the moment of misalignment — they're the
      ground truth for "in what state did we get this wrong" once
      we have real-world traces.
    - Persistence: optional. A rolling 7-day decision log keyed by
      context-bucket could feed an offline tuning notebook, but
      that's a v2 step. V1 is "make decisions based on context",
      not "learn from history" — though it pairs naturally with
      the V32 `NetworkStats` persistent profile work already filed
      below.
  This is real new architecture, not file shuffling. Estimate 2–3
  focused sessions: 1 for the AppContext + provider, 1 for wiring
  consumers and pruning the now-redundant ad-hoc fall-back paths,
  1 for verification + tuning passes on real-device logs.

  **V32.21 status — V1 shipped:**
    - ✅ `AppContext` struct + `AppContextProvider` QObject in
      `core/AppContext.{hpp,cpp}` (described above)
    - ✅ Wired via `AppController::appContext()`, attached to
      `QGuiApplication::applicationState` and `QNetworkInformation`
      transport/metered changes
    - ✅ `noteUserAction()` fed from `snapCaptured` + `snapOpened`
    - ✅ `noteReachabilityFlap()` + `noteSsidChange()` from existing
      AppController paths
    - ✅ `setCurrentPage()` from `SwipeNavigator::pageChanged`
    - ✅ First consumer: `NatTraversal::initiateHolePunch` halves the
      burst count (60→30 packets) when context is conservative + user
      idle, full burst when `userJustActed`
  **Follow-ups for the next FSM-app-state session:**
    - [x] ~~Instantiate `PowerManager` somewhere in `AppController::initialize`
      and call `d->appContext->attachPowerManager(d->power.get())` —
      currently `screenOn` / `onAc` / `powerSaver` are stuck at
      optimistic defaults~~ **Done V32.34+.** PowerManager is now
      constructed in `AppController::initialize()` and attached to
      AppContext. DhtResourceManager is also instantiated and wired to
      it (power-aware 4-tier DHT mode), and CameraWidget subscribes to
      `screenActiveChanged` to stop the pipeline on screen-off. See the
      new "Battery management" section below for the full plan + remaining
      sub-items.
    - [x] ~~Wire `UdpChannel` retransmit-timer interval to context:
      stretch retransmit cadence when `prefersConservative()` and
      shorten when `userJustActed(2000)`~~ **Done V32.28.** UdpChannel
      now has `setAppContext` following the network-consumer pattern.
      `onRetransmitTimer` increments a tick counter and, when
      `prefersConservative() && !userJustActed(5000)` and the tick is
      even, skips the chunk-retransmit loop (effective 3s cadence vs
      1.5s). CONNECT retransmits stay at full cadence regardless —
      relay fallback timing must remain bounded.
    - [x] ~~Wire AppController reconnect scheduler: immediate reconnect
      when `userJustActed`, exponential when idle~~ **Done V32.28.**
      The peerDisconnected handler in `AppController_connect.cpp`
      now picks reconnect delay based on context: 1000ms when
      `userJustActed(2000)` (user is actively trying to send/view —
      they're waiting), 10000ms (the previous fixed default) otherwise.
      The 2s userJustActed window matches the shutter→send latency
      so a snap-fail-then-reconnect doesn't race against a stale
      "user is idle" reading. Exponential backoff for repeated
      failures is still TBD; the current jump is bimodal not graded.
    - [x] ~~DHT bootstrap retry interval reads
      `reachabilityRecentlyFlapped(5000)` and stretches when true (no
      point bootstrapping while network is half-broken)~~
      **Done V32.28.** `DhtDiscovery::setAppContext` follows the
      established pattern; `start()` now derives `bootstrapDelayMs`:
      30s default, 60s when `reachabilityRecentlyFlapped(5000)`.
      The flap window prevents the retry from firing during a network
      transition when UDP probes would just be dropped. AppController
      wires `dhtDisc->setAppContext(d->appContext.get())` after
      DhtDiscovery construction.
    - [x] ~~LAN-isolation threshold (currently 2 from V32.15) drops to 1
      when `connectionType == Cellular` — LAN literally cannot work~~
      **Done V32.27.** `NetworkManager` now has `setAppContext`
      following the V32.21 NatTraversal pattern; the LAN-isolation
      gate in `connectToPeer` derives `effectiveThreshold = 1` when
      `ctx.connectionType == Cellular`, else falls back to the static
      `kIsolatedLanThreshold = 2`. Log line includes the
      "(threshold lowered: cellular detected)" suffix when adapted
      so device traces are unambiguous.
    - [x] ~~FSM misalignment logs include AppContext snapshot — required
      for offline tuning to make sense of bad-state traces~~
      **Done V32.28.** `FSMValidator::logMisalignment[RateLimited]`
      gained an optional `AppContextProvider*` parameter. When non-
      null, the log line appends a bracketed summary like
      `ctx[appActive=1,screenOn=1,page=camera,conn=wifi,metered=0,
      powerSaver=0,onAc=1,userAct=1,ssidRecent=0,reachFlap=0]` —
      single bracket, no inner spaces, easy to grep for specific
      conditions ("metered=1" finds cellular cases, "appActive=0"
      finds backgrounded misalignments, etc.). UdpChannel passes
      its `m_appContext`; future FSM consumers follow the same
      pattern.

- [ ] Persistent per-peer connectivity profile: remember
  "Alice always needs relay" so we don't re-discover on every
  startup. **V32 partial:** in-memory `NetworkStats` module records
  data per session; needs StorageManager persistence layer to survive
  app restart. Add table `network_stats(peer_id, address_class,
  attempts, successes, median_ms, last_success_ms)` + load on startup
  + save periodically.
- [x] ~~NetworkStats reranking is only applied on cache-hit code path
  (`connectByDhtId` in `AppController_commands.cpp`). The live-DHT-
  lookup path also gets candidate addresses — it should consult
  NetworkStats too for symmetric behaviour.~~ **Done V32.25.**
  Extracted the inline rerank into `applyNetworkStatsRerank(addrs,
  peerId, callSite)` helper in the anon namespace. Called from both
  the cache-hit path AND the live-DHT singleShot deferral just
  before `tryNextPeerAddress`. Logs include the `callSite` tag
  (`"cache-hit"` vs `"live-DHT"`) so real-device traces can confirm
  both fire. Cold-start case should now behave like steady-state.
- [x] ~~Settings page UI for live-toggling DebugFlags~~ — done in V32.7
  (NoFsmTuning, NoMonitoringBlocking, VerboseFsm). Live-apply on
  toggle, persistence on Save. Open question still: should this be
  visible to all users or behind a 7-tap secret? Currently visible.
- [ ] NetworkStats score-function weights (currently 0.6/0.25/0.15
  for rate/recency/latency) are guesses. Once we have real-world
  data, tune these. Maybe surface as an experiment flag.
- [ ] IPv6 path support (NetworkManager / NatTraversal both v4-only)
- [ ] Multi-path candidate gathering — try LAN AND public AND relay
  in parallel; first connect wins, others abort
- [ ] DHT mailbox TTL is only 10 min; increase or add longer-lived
  store-and-forward
- [ ] WebSocketTransport pong-tracking force-disconnects after 90s
  of no answer — verify reconnect logic actually picks alt addresses

### UI / UX
- [~] **In-page tabbed surfaces — breadth pass done V32.34+, finalize
  pending.** Goal: consistent in-page tab pattern across Settings,
  Mailbox, and Contacts. Approach was breadth-first — establish the
  pattern, propagate the basics to all three, then deepen each.
  - **Shared widget:** `SegmentedTabStrip` (include/ui +
    src/ui) — a row of equal-width checkable buttons with an underline
    indicator, `tabSelected(int)`, `setBadge(i,count)`, `setLabel`.
    Extracted from the inline strip first written in tabbed Settings.
  - **Settings:** 5 tabs (General / Camera / Network / Storage / About)
    over a QStackedWidget; sticky save-bar retired in favor of an
    always-visible action bar. (Still uses its own inline strip — a
    finalize task is retrofitting it onto SegmentedTabStrip for full
    consistency.)
  - **Mailbox (InboxWidget):** All / Unread filter tabs replace the
    static "Recent snaps" label; Unread tab badge = unread-conversation
    count; filtered-empty state handled.
  - **Contacts (ContactsPage):** All / Online / Pending tabs compose
    with the existing text search; Online tab badge = connected-peer
    count; filter-aware empty text. **Depth pass V32.34+:** each card
    now carries a conversation-activity line ("last snap <reltime>",
    with "N new ·" prefix + accent/bold when unread), pulled from
    StorageManager::loadConversations and refreshed (debounced 150ms) on
    snapReceived/snapDelivered via MainWindow. Cards read as conversation
    summaries, not just presence rows. (Direction glyph ↙/↗ intentionally
    omitted — ConversationSummary exposes hasInbound/hasOutbound but not
    the most-recent snap's direction, so showing one would be false
    precision; revisit if a lastIsOutbound field is added to the summary.)
  - **Finalize TODO (deepen pass):** (1) ~~retrofit Settings onto
    SegmentedTabStrip~~ **done V32.34+** — Settings now uses the shared
    strip (its bespoke inline button row is gone; selectTab drives only
    the stack, the strip owns its own highlight via tabSelected);
    (2) ~~remember last-open tab per surface (QSettings)~~ **done
    V32.34+** — SegmentedTabStrip gained setPersistenceKey(key): when set
    it restores the saved index on construction (emitting tabSelected so
    the host syncs) and writes the index on every change. Wired with keys
    ui/tabs/settings, ui/tabs/mailbox, ui/tabs/contacts, each called at
    the end of the host's buildUi (after the dependent stack/list widgets
    exist and after the tabSelected connect); (3) ~~badge
    first-paint position bug~~ **done V32.34+** — SegmentedTabStrip now
    repositions badges in resizeEvent + showEvent (rebuildBadge split
    into create/set-text vs. repositionBadge), so the count pill lands
    top-right from first paint instead of clamping to x=0 until the next
    rebuild; benefits all three surfaces at once now that they share the
    widget; (4) ~~mailbox "Sent" tab~~ **done V32.34+**; (5) optional tab
    icons.

- [ ] **🌟 Mobile: move tab strip to the BOTTOM and merge it with a
  contextual action row into one "smart function ribbon" (user directive
  V32.34+).** On mobile (narrow / Plasma Mobile / FP5), the in-page
  `SegmentedTabStrip` should live at the BOTTOM of the screen for thumb
  reach, and it should fuse with a context-action row so the same bar
  shows BOTH tab-switching AND the actions relevant to the active
  tab/subtab — adapting like a ribbon rather than staying a static tab
  strip. Examples of the contextual half: Contacts→Pending shows
  accept-all / clear-all; Mailbox→Sent shows retry-failed / clear-
  delivered; Mailbox→Unread shows mark-all-read. The tab half stays
  constant per surface; the action half swaps per selected tab.

  **This is an architectural change, not a tweak — design before code.**
  **Decisions settled (user, V32.34+):**
  - **Layout = ONE ROW** — tab segments on the left, contextual actions on
    the right, in a single bottom bar (best for the 380px height budget).
  - **REPLACE the page-nav TabBar with one unified bottom bar** — at a page
    root the bar shows page navigation (Camera/Inbox/Friends/…); inside a
    tabbed surface it MORPHS to show that surface's tabs + the active tab's
    actions. This is the natural extension of the existing "stack-style
    navigation" precedent (drilling in swaps the bottom bar's tabs).
  - **Prototype on ONE surface first, then propagate** — Inbox/Mailbox
    chosen (richest action set → best stress test).

  **Build plan (prototype-first, do NOT touch the shared TabBar yet):**
  Because "replace the global TabBar" is a change to a central, well-tested
  navigation widget, the prototype builds a standalone `SmartRibbon` and
  proves the one-row tab+action layout on Inbox in isolation BEFORE
  rewiring MainWindow's bottom bar. Sequence:
    1. ~~New `SmartRibbon` widget~~ **built V32.34+** —
       `include/ui/SmartRibbon.hpp` + `src/ui/SmartRibbon.cpp`, registered
       in src/ui/CMakeLists.txt. One row: left = natural-width checkable
       tab segments, right = the active tab's contextual action buttons
       (icon-optional, with enabled-predicate + trigger callbacks).
       Bottom-docked: accent indicator + count badges paint on the TOP
       edge (facing the page). Reuses SegmentedTabStrip's visual language +
       persistence/badge semantics (setBadge/setLabel/setPersistenceKey).
    2. ~~Prototype it inside the Inbox page (opt-in)~~ **built V32.34+,
       PENDING DEVICE TEST** — InboxWidget builds a bottom SmartRibbon
       behind `smartRibbonEnabled()` (env `SC_SMART_RIBBON=1` or QSettings
       `ui/smart_ribbon`). When on, the top SegmentedTabStrip is hidden and
       the ribbon drives the filter; both go through a new shared
       `applyFilterIndex(idx)` so behavior is identical, and badges update
       via a shared `setFilterBadge` that hits both strips. Contextual
       actions: Unread→"Mark all read" (`markAllConversationsRead`, enabled
       when unread>0), Sent→"Refresh" (`reloadConversationsFromStorage`,
       enabled when outbound present — a true retry-failed path isn't in
       the inbox yet so the prototype uses the real reload rather than a
       dead signal). Persistence still flows through the hidden top strip
       (one source of truth) and syncs the ribbon. DEVICE-VALIDATE: enable
       the flag, check the bottom bar's thumb-reach + that it doesn't
       crowd the list at 380px height; confirm tab switching + the two
       actions work and the indicator/badges land on the top edge.
    3. Only after step 2 is device-validated: fold it into the global
       TabBar so the one bottom bar morphs page-nav ↔ tab+actions.
    4. Propagate to Contacts, then Settings.

  Filed-as-spec design notes (still open for the later phases):

  - **Layout of the merged bar.** Options: (a) one row, tabs on the left
    and contextual actions on the right; (b) two stacked rows, tab strip
    above an action row; (c) tabs by default, actions revealed on
    long-press / swipe-up; (d) tap the active tab again to expand its
    actions inline. (a) is most compact (best for 380px height budget);
    (b) is clearest but eats vertical space the page needs; (c)/(d) hide
    actions behind a gesture (discoverable?). Leaning (a) for the height
    budget, but unresolved.
  - **Relationship to the existing page-nav `TabBar`.** The app already
    has a bottom-ish page navigation TabBar (Camera / Inbox / Contacts /
    …). Does the new ribbon (1) REPLACE it with a single unified bottom
    bar that shows page-nav when at a page root and morphs into
    tab+actions when inside a tabbed surface; (2) SIT ABOVE it as a second
    bar (two stacked bars — costly on height); or (3) MERGE into the
    page-nav as a second tier? Note the existing pattern at
    `ConversationDetailDialog` already REPLACES the bottom TabBar's tabs
    when drilled in (see the entry near line 751) — option (1) is the
    natural extension of that precedent.
  - **Desktop/wide behavior.** Keep the strip at the TOP on wide
    viewports (current behavior) and only relocate to the bottom under
    the mobile breakpoint, OR unify on bottom everywhere. The
    `SegmentedTabStrip` would need a `setEdge(Top|Bottom)` (or the host
    decides placement) so the same widget serves both; the underline
    indicator flips to the top edge when bottom-docked.
  - **Scope/sequencing.** Prototype on ONE surface first (Mailbox is the
    richest action set → best stress test) then propagate, vs. do all
    three together. Given it's architectural, a one-surface prototype that
    proves the layout + TabBar relationship before propagating is the
    safer path.

  **Widget shape (tentative).** Extend `SegmentedTabStrip` (or wrap it in
  a new `SmartRibbon` composite) so a host can register, per tab index, a
  small set of contextual actions (icon + label + enabled-predicate +
  callback). The ribbon renders the tab segments plus the active tab's
  actions, re-querying the action set on `tabSelected`. Keep the
  persistence (last-open tab) and badge logic already in
  SegmentedTabStrip. Reuse Theme TOUCH_MIN for hit targets;
  bottom-docked means the indicator and any badges move to the top edge
  of the bar. DEVICE-VERIFY heavily at 380px — this bar competes with the
  page content for a tight vertical budget, and thumb-reach is the whole
  point, so it has to be tested in-hand, not just reasoned about.

  **V32.34+ height-fix (device feedback).** With `SC_SMART_RIBBON=1` the
  Sent/Unread action buttons rendered too tall. Cause: bar items used
  `setMinimumHeight(TOUCH_MIN)` + `Fixed` policy, so stylesheet vertical
  padding inflated each button's sizeHint and the tallest stretched the
  bar. Fix: every bar item (segments AND action buttons) is now
  `setFixedHeight(TOUCH_MIN)`, action-button vertical padding dropped (the
  fixed box owns the height), and the ribbon widget itself is capped to
  `TOUCH_MIN + 2*SPACE_XS + 1px border`. Also applied the requested cap to
  `SegmentedTabStrip` (max height = content + touch padding) so the top
  strip can't claim slack vertical space either.

  **── PHASE 3+ DIRECTIVE: unified bar + two-level horizontal swipe
  navigation (user, V32.34+) ──**
  Confirmed intent to "fully implement the unified bar and merge the two."
  The model the user sketched:
  `|| CONTACTS . C1 . C2 . SENT . UNREAD | NEXT PAGE ||` — ONE bottom bar
  showing the current page's sub-tabs inline, with a "NEXT PAGE"
  affordance at the right edge. Navigation:
  - **Horizontal swipe scrubs the sub-tabs** within the current page.
  - **At the end of the sub-tabs, a friction detent** (extra resistance +
    padding) — pushing PAST it advances to the NEXT PAGE. One continuous
    horizontal gesture moves through sub-tabs, then "breaks through" the
    detent to the next page. (This is the two-level navigation: level 1 =
    sub-tabs, level 2 = pages, with a tactile boundary between them.)
  - **Desktop equivalents:** Ctrl+Tab / arrow keys drive the same
    traversal; OR a trackpad two-finger horizontal gesture once the cursor
    has focus in the bar's subregion. Plug into the EXISTING tap-drag
    system (SwipeNavigator's delta plumbing) rather than a parallel path.

  **OPEN DESIGN QUESTIONS (must settle before coding — they fork):**
  - End-of-sub-tabs behavior: (a) sub-tabs cycle, then friction detent
    breaks to next page; (b) hard stop at last sub-tab, NEXT PAGE button is
    the only page jump; (c) continuous — sub-tabs flow straight into the
    next page's sub-tabs with no cycle; (d) swipe = sub-tabs only, page
    change stays a separate (vertical?) gesture.
  - Landing sub-tab when crossing to the next page: first sub-tab always /
    last-used (remembered) / carried-over index.
  - Does this horizontal model REPLACE the current page-swipe gesture
    (SwipeNavigator's horizontal page paging), COEXIST with it, or does
    page-nav stay tap-only on the bar while horizontal = sub-tabs only?
    NOTE: SwipeNavigator currently uses HORIZONTAL swipe for page paging —
    so "horizontal swipe scrubs sub-tabs" DIRECTLY CONFLICTS with the
    existing gesture unless the two-level detent model subsumes it (sub-tab
    scrub IS the first level, page paging IS the break-through second
    level). This is the crux: the cleanest read is that the new model
    REPLACES SwipeNavigator's flat page-paging with a two-level version
    where the same horizontal axis does sub-tabs first, pages second. That
    is a real change to SwipeNavigator, not just the bar — needs its own
    session + careful gesture-tuning + on-device feel testing (friction
    coefficient, detent width, momentum hand-off).

  **DECISIONS SETTLED (user, V32.34+):**
  - **End-of-sub-tabs = CYCLE then DETENT.** Horizontal swipe cycles the
    sub-tabs; at the cycle boundary a friction detent breaks through to the
    adjacent PAGE.
  - **Landing = ALWAYS the next page's FIRST sub-tab** (index 0). Simple,
    predictable; no remembered/carried index.
  - **Two-axis model — REFINED (user, V32.34+).** The axes have DISTINCT
    roles (not redundant):
    * **VERTICAL = content scroll, with an end-of-content detent that
      advances the SUB-TAB.** Scrolling through messages / the settings
      list is normal vertical scroll; when you reach the content's natural
      end (overscroll past the bottom) a detent fires that nudges to the
      NEXT sub-tab — so a long thread or settings list flows into the next
      sub-tab rather than dead-stopping. (Like pull-to-refresh, but the
      payoff is "advance sub-tab".)
    * **HORIZONTAL = move between sub-tab focus** (the submenu level),
      cycling, then detent-breaks to the next PAGE (per above).
    * **TAP = jump directly to a sub-tab.**
  - **Detent FEEL = lightweight barrier with visual nonlinearity** — the
    resistance builds (sub-linear), and a **barrier icon/SVG color-fades
    in** at the boundary as the user pushes against it, signaling the
    threshold before it gives.
  - REMAINING forks being confirmed (see asks): exactly WHEN a vertical
    drag counts as detent vs. content scroll (overscroll-past-end is the
    leading read); whether UP-past-top does anything; and whether the
    barrier visual is shared across axes or per-axis.

  **FINAL DECISIONS (user, V32.34+) — design COMPLETE, ready to build:**
  - **Detent ARMS only after scroll hits its limit** in that direction.
    Normal content scroll is never interrupted; the detent is purely an
    end-of-content (overscroll) outcome. So the vertical drag counts as
    "detent" ONLY once `verticalScrollBar` is at max (down) / min (up) and
    the user keeps pulling.
  - **~1s context-switch window (NOT 400ms).** After the scroll reaches the
    limit, there's a ~1000ms window during which continued/renewed pull
    crosses the detent and advances the sub-tab. The longer window (vs.
    400ms) is deliberate: it makes a FLICK that reaches the end and a
    follow-through pull feel like ONE unified motion, rather than two
    disconnected gestures, and stops a fast momentum fling from instantly
    blowing through into the next sub-tab. Implementation: when overscroll
    begins (limit hit), start/refresh a ~1s "detent-armed" clock; the
    sub-tab advance only commits while armed AND past the distance
    threshold. After the window lapses with no threshold cross, it
    disarms and the next pull starts the window fresh.
  - **Vertical direction = SYMMETRIC.** Down past bottom → NEXT sub-tab;
    Up past top → PREV sub-tab.
  - **Barrier visual = ONE generic barrier to start**, specialize per-axis
    later. Single shared SVG/overlay that fades in proportional to detent
    progress on whichever axis is active.

  **SwipeNavigator integration map (groundwork read, V32.34+).**
  `src/ui/SwipeNavigator.cpp` (~2495 lines) + hpp (~617). The two-level
  state machine extends THIS, it does not replace it. Key existing
  machinery the detent model hooks into — do NOT rebuild, extend:
  - Gesture core: `gestureBegin/Update/End`; `m_slideOffset`,
    `m_dragDistance`, `m_pageWidth`, `m_currentIndex`. Page commit/bounce:
    `startCommitAnimation` / `startBounceAnimation`, `rubberBand(drag,
    pageW)` for edge resistance — the HORIZONTAL page-break detent is a
    natural extension of this rubber-band math.
  - **Overscroll system (V32.34+, the KEY hook for the VERTICAL detent).**
    SwipeNavigator already detects drag/fling past the scroll end and
    rubber-bands the viewport: `m_overscrollRawY` (accumulated raw delta
    past the edge), `m_overscrollY` (rubber-banded offset, curve
    `sign×sqrt(|raw|×viewport×kOverscrollRubberK)` — sub-linear, "pull
    more gain less"), spring-back over `kSpringBackMs` (220ms),
    `kOverscrollMaxRatio` clamp. The VERTICAL sub-tab detent is a clean
    extension: as `m_overscrollRawY` grows past the content end, fade in
    the barrier SVG proportional to overscroll progress; if release (or
    sustained pull) crosses a detent threshold, ADVANCE the sub-tab instead
    of (or in addition to) springing back. The existing rubber-band curve
    IS the requested "visual nonlinearity"; no new resistance math needed.
  - Intent system (V32.33+): `classifyIntent()` picks ONE intent at
    TouchBegin and it persists — `GestureIntent` { PassToChild,
    ChildScrollV, ChildScrollH, PageSwipe, TapCommit, Ambiguous }. The
    vertical detent lives INSIDE the existing `ChildScrollV` intent (it's
    an overscroll outcome, not a new intent → no mid-stroke reclassify, no
    jitter regression). The horizontal page-break stays within the
    PageSwipe/sub-tab-scrub intent. CRITICAL: do not reintroduce mid-stroke
    intent flips — the detents are COMMIT-TIME outcomes of an already-
    chosen intent.
  - Desktop inputs already present: `keyPressEvent`, `wheelEvent` → wire
    Ctrl+Tab / arrows / trackpad two-finger onto a shared
    `advanceSubOrPage(axis, dir)` entry point that gesture-commit AND
    key/wheel handlers call.
  - The unified bar (SmartRibbon) owns the (page, subtab) index, renders
    sub-tabs inline + a next-page affordance, and is the barrier-icon's
    host (or a shared overlay). SwipeNavigator queries it for the current
    page's sub-tab count (cycle boundary) and pushes the active sub-tab as
    the gesture scrubs.

  **Implementation notes (tentative, post-decision):**
  - **STEP A done V32.34+: the `SubTabbed` interface + ALL THREE surfaces.**
    `include/ui/SubTabbed.hpp` — a pure interface (subTabCount /
    currentSubTab / setCurrentSubTab / subTabLabel / subTabBadge) that
    pages implement so SwipeNavigator + the unified bar drive sub-tabs
    generically (no per-page coupling). Implemented on all three tabbed
    surfaces, each multiply-inheriting QWidget+SubTabbed (QWidget first as
    Qt requires) and routing through a shared apply*Index helper so a tap /
    horizontal scrub / vertical detent all funnel through the SAME filter
    path the in-page strips use:
      * InboxWidget — All/Unread/Sent via m_convFilter/applyFilterIndex;
        badges = unread-conv / in-flight-outbound counts.
      * ContactsPage — All/Online/Pending via m_cardFilter/
        applyCardFilterIndex; badges = online / ContactState::Pending
        counts (matches rebuildCards bucketing exactly).
      * SettingsPage — General/Camera/Network/Storage/About via
        selectTab over the QStackedWidget; subTabCount reads the live stack
        count, null-guarded for pre-buildUi calls.
    This is the pure-state foundation (no gesture-timing feel) the rest
    builds on, verifiable by inspection. NEXT (still no device needed):
    a small `QList<SubTabbed*>` registry or dynamic_cast in MainWindow so
    the navigator can ask the current page for its sub-tab model; THEN the
    gesture state machine (the device-tuned part).
  - **STEP B done V32.34+: SwipeNavigator → SubTabbed bridge.** Rather than
    a registry in MainWindow, the navigator (which already owns the page
    widgets via m_pages) reaches the current page's sub-tab model by
    `dynamic_cast<SubTabbed*>(currentPage())`. New public API (pure
    delegation, NO gesture timing): `currentSubTabbed()` (null if the page
    has none), `currentSubTabCount()`, `currentSubTabIndex()`,
    `advanceSubTab(delta)` (non-wrapping; returns false at an edge so the
    caller knows to fire the page-break detent), and `cycleSubTab(delta)`
    (wrapping; returns -1/0/+1 so the caller fires the detent exactly on a
    wrap). Verified the cross-cast: only the 3 tabbed surfaces inherit
    SubTabbed, so Camera/Wayfinder dynamic_cast to null and the two-level
    logic degrades cleanly to flat page paging there. This is the last
    inspection-verifiable piece. NEXT = the gesture state machine: the
    device-tuned part (1s armed window, symmetric vertical detent,
    horizontal cycle-then-break, barrier-fade), built behind
    SC_UNIFIED_NAV with a build-feel-adjust loop on the phone.
  - **STEP C done V32.34+ (gesture state machine — NEEDS DEVICE FEEL-TUNING).**
    Built in SwipeNavigator behind `unifiedNavEnabled()` (env SC_UNIFIED_NAV=1
    or QSettings ui/unified_nav), INERT otherwise:
      * VERTICAL detent — `evaluateVerticalDetent(rawY)` hooked into
        applyOverscroll. Arms on first overscroll past the content limit,
        ~1s armed window (re-arms on renewed pull so flick+pull feels
        unified), fires ONE advanceSubTab per streak when |rawY| crosses
        kVDetentRawThreshold. Symmetric: down=next, up=prev. Resets
        overscroll on fire so the new sub-tab starts clean.
      * HORIZONTAL two-level — in gestureEnd, a commit-worthy swipe cycles
        the sub-tab (cycleSubTab); only when drag exceeds
        pageW*kCommitFraction + kHDetentBreakPx (the friction) does it break
        through to a page change. Landing on a new page resets to its first
        sub-tab (in goToPage + startCommitAnimation finish callbacks).
      * KEYBOARD — keyPressEvent: arrows / Ctrl+Tab / Ctrl+Shift+Tab
        advance sub-tab (non-wrapping) then break to adjacent page at the
        edge. The desktop mirror of the swipe.
      * BARRIER overlay — paintEvent draws an edge-aware material glow that
        fades in proportional to m_barrierOpacity (detent progress).
        V32.34+ UPGRADE: now a soft LinearGradient glow on the EDGE matching
        the active detent (m_barrierEdge: 0=bottom 1=top 2=right 3=left) —
        vertical end-of-content detent glows top/bottom, horizontal
        sub-tab→page friction glows on the correct SIDE — with thickness +
        alpha both tracking progress so the boundary "firms up" as you push.
      * WHEEL / TRACKPAD friction — V32.34+: horizontal mouse-wheel /
        trackpad already routed through gestureBegin/Update/End (existing
        processWheelEvent), so it inherits the two-level commit for free. ADDED
        the felt friction: in gestureUpdate, when the page is SubTabbed, the
        drag is 1:1 up to the sub-tab commit point, then sub-linear sqrt
        RESISTANCE builds through the friction zone toward the page-break
        distance (kHDetentBreakPx), with the barrier glow ramping in. So the
        boundary between "scrubbing sub-tabs" and "flipping the whole page" is
        resisted + shown, not just timed. Material feedback = the
        OutCubic (fast-out/slow-in) snap-back on scrub-commit; barrier clears
        on every gesture end.
    FEEL CONSTANTS (kDetentArmWindowMs=1000, kVDetentRawThreshold=220,
    kHDetentBreakPx=90) are SEEDS — they MUST be tuned in-hand on the FP5;
    friction/threshold/timing can't be reasoned to correctness. Trackpad
    two-finger maps through the existing wheelEvent path and now gets the
    friction + barrier too. Possible follow-up: animate the barrier FADE-OUT
    (currently instant-clears) and a material content transition on the
    sub-tab swap itself (lives in the page, not the navigator) — deferred as
    gold-plating until the core feel is validated.
    CAVEAT: built in one pass; a function-insertion edit accidentally
    orphaned setSlideOffset (the slideOffset Q_PROPERTY's WRITE setter,
    driven ~60Hz by every page animation) — caught by a strict brace check
    and restored. Worth a careful first compile to confirm nothing else
    shifted.
  - The bar becomes the single source of "where am I" — renders the active
    page's sub-tabs inline + a next-page affordance, owns the (page,
    subtab) index.
  - **UnifiedNavBar BUILT V32.34+ (the visible bar).** `include/ui/
    UnifiedNavBar.hpp` + `src/ui/UnifiedNavBar.cpp` (Q_OBJECT). View-only over
    SwipeNavigator: renders [‹ prev][PAGE NAME][sub-tab segments…][next ›].
    rebuildForCurrentPage() on pageChanged (reads currentSubTabbed model, page
    arrows enabled by index bounds), refreshActiveSegment() on subTabChanged,
    tap a segment = direct jump (setCurrentSubTab). Active segment uses
    ACCENT_SOFT bg + ACCENT text. Registered in src/ui/CMakeLists.txt (cpp +
    AUTOMOC header). MOUNTED in MainWindow: when unifiedNavEnabled(),
    `m_unifiedBar = new UnifiedNavBar(m_swiper, wrapper)`, m_tabBar->hide(),
    add unifiedBar to the layout instead of tabBar; else add tabBar as before.
    Toast positioning now uses the active bottom-bar height (hidden TabBar
    reports stale height). Added `pageName(int)` + m_pageNames to
    SwipeNavigator (addPage previously DISCARDED the name — that's why the bar
    needed it added). Non-SubTabbed pages (Camera/Wayfinder) show just the
    page name + arrows (no segments). Added `subTabChanged(int)` signal so the
    bar updates its highlight without polling.
  - **UNIFICATION CLEANUP V32.34+ (top bars removed, padding, responsive
    collapse).** Three follow-ups once the bottom bar was live:
      * TOP BARS HIDDEN — each SubTabbed page (InboxWidget m_filterTabs,
        ContactsPage m_filterTabs, SettingsPage m_settingsTabs) now HIDES its
        in-page SegmentedTabStrip when unifiedNavEnabled(), since the bottom
        bar renders the same sub-tabs. HIDE not DELETE on purpose: (a) the
        strip's tabSelected→apply…Index wiring + persistence stay live; (b)
        deleting styled widgets is exactly what tripped the QStyleSheetStyle
        teardown crash, so we don't add more of that. The bottom bar's segment
        taps drive setCurrentSubTab→apply…Index, so behavior is identical.
      * PADDING — bar height cleaned to exact TOUCH_MIN + 2×SPACE_XS (dropped
        the stray +1); inner controls pinned to TOUCH_MIN, SPACE_XS top/bottom
        is the breathing room. Segment styling factored into one segmentStyle()
        helper so the active-refresh and collapse restyle can't drift.
      * RESPONSIVE COLLAPSE (active-keeps-label rule) — applyResponsiveCollapse()
        runs on resize + after rebuild, picks a level by measured width:
        L0 wide = page-name chip + all full labels; L1 = drop the page-name
        chip; L2 = active keeps full label, inactive shrink to compact padding;
        L3 = active keeps full label, inactive → icon (SubTabbed::subTabIcon, a
        new OPTIONAL hook defaulting empty) or a 3-char short form. HONEST
        LIMIT: most sub-tabs (All/Unread/Sent, Online/Pending) have NO icon
        asset — there are only page-level SVGs (camera/inbox/friends/settings/
        send/…). So "icons for everything" isn't achievable without authoring
        ~13 new sub-tab SVGs; the achievable rule uses icons WHERE a page maps
        a sub-tab to an existing asset (e.g. Settings "Camera"→"camera") and a
        short-text fallback otherwise. The subTabIcon hook is in place so those
        mappings can be added per-page later, no further bar changes needed.
  - **EXPANDABLE CONTACT CARD + SWIPE-OUT SUBMENU V32.34+ (new feature).**
    New widget ContactCard (src/ui/ContactCard.{hpp,cpp}, Q_OBJECT, registered in
    src/ui/CMakeLists.txt cpp-list + AUTOMOC header) that wraps a contact's
    existing header row and adds a height-animated action submenu:
      * TAP the header → expand/collapse the submenu (QPropertyAnimation on a
        submenuHeight property, 180ms OutCubic; submenu maxHeight clipped).
      * SWIPE-OUT (horizontal drag ≥48px, x-dominant) on an expanded card →
        collapse. A near-zero-travel release = tap (toggle); a larger non-armed
        move = scroll/no-op (doesn't toggle). Thresholds: kTapSlopPx=10,
        kSwipeCloseThresholdPx=48.
      * Header event-filtered for the gesture; child buttons (inline edit /
        share-loc) still work — they consume their own click before the filter
        sees a release as a tap.
    SUBMENU ACTIONS (ContactsPage::makeCard wires them): Edit contact (same
    openEditor path), Share my location (same locationShareRequested signal),
    Copy ID (clipboard), Remove contact (red/danger; inline QMessageBox confirm
    → removeContact + contactDeleted, reusing the editor's delete codepath — no
    new signal). Single-open-at-a-time: ContactCard::expanded(self) → page
    collapses the previously-open one (tracked via QPointer m_openCard, so a
    rebuildCards() that deletes the open card can't leave a dangling deref).
    OBJECTNAME FIX (compile/runtime-blind hazard caught by inspection): the
    inner header was renamed contactCardHeader_<id> while the OUTER ContactCard
    takes contactCard_<id> — otherwise findChild<QWidget*>("contactCard_<id>")
    (used by setConnectionStatus / card-removal) would depth-first match the
    inner header and removeWidget the wrong node, orphaning the wrapper. Inner
    status/badge/netstat labels are still descendants so their recursive
    findChild lookups still resolve. NEEDS DEVICE: tap-expand + swipe-close feel
    (the 48px swipe threshold is a seed); confirm it doesn't fight the page's
    own vertical scroll or the unified nav's horizontal page-swipe.
    ▶ V32.34+ GESTURE HARDENING (reduce scroll/page-swipe conflict, pre-device):
      - MouseMove now THREE-way: if vertical motion dominates first (ay>ax,
        ≥10px) the card bows out (stops tracking, doesn't consume) so the page
        scroller keeps the gesture; swipe-close requires CLEAR horizontal
        dominance (ax ≥48px AND ax > ay*1.5) before it arms+collapses+consumes
        (the consume stops the same drag also being read as a page swipe); an
        UNEXPANDED card stops tracking on any real motion so a scroll never
        toggles it on release.
      - Submenu height for the open animation now read from the LAYOUT's
        sizeHint (margins included), not the widget's — the widget's sizeHint
        was skewed by the maximumHeight(0) clamp used to keep it collapsed.
      - Navigate-away cleanup: ContactsPage::hideEvent collapses m_openCard, and
        layoutPages() genuinely setVisible(false)s off-screen pages (verified),
        so swiping away from Contacts fires hideEvent → the card collapses
        instead of lingering open on return.
    ▶ V32.34+ ACTIONS AUDIT + CHEVRON + SETTINGS MOVE:
      - Audited the 4 submenu actions: Edit→openEditor(QString) (sig matches),
        Share→locationShareRequested signal, Copy ID→clipboard, Remove→inline
        QMessageBox confirm then removeContact+contactDeleted. Use-after-free
        check on Remove: removeContact→rebuildCards deletes cards via
        deleteLater() (NOT delete), so the card object outlives the action
        lambda's trailing collapse() call — safe. m_openCard is QPointer so it
        auto-nulls when the deferred deletion lands.
      - Added an expand CHEVRON affordance: ContactCard::paintEvent draws a
        subtle ▾ (collapsed) / ▴ (expanded) at the header's right edge so the
        card reads as tappable-to-expand. Painted on the card (not a header
        child) so it doesn't disturb the caller-built header layout; repaints on
        expand/collapse via update(). NOTE: chevron sits at width()-14, which
        could overlap the inline edit/loc buttons if those stay — final position
        depends on the pending "declutter inline buttons?" decision (asked).
      - Unified-nav toggle RELOCATED from About-tab debug flags to Settings →
        General → Appearance → NAVIGATION (a navigation-style choice belongs in
        Appearance, not buried in dev diagnostics). Single instance confirmed;
        persists immediately + inline restart note.
  - **BOUNCE-ON-TAP FIX + OVERSCROLL DIAGNOSTIC V32.34+.** Established the
    "weird bounce on tap" is NOT a unified-nav artifact — the visual overscroll/
    spring-back path (applyOverscrollToViewport + startSpringBack) is ALWAYS-ON
    (only the detent FIRING at the end of applyOverscroll is flag-gated). Root
    cause found by inspection: flushPendingScroll() runs on EVERY TouchEnd incl.
    a tap, and a tap can leave a few px of finger jitter in m_pendingScroll*
    that never crossed the 14px axis-lock to become "real" scrolling. At a
    scroll edge that residual became overflow → applyOverscroll → spring-back =
    the bounce. FIX: flushPendingScroll now only commits if m_scrollCommitted
    (a real scroll engaged this gesture) OR the pending delta exceeds a small
    tap-jitter floor (3px, well under the 14px lock so genuine micro-scrolls
    aren't lost); pure tap jitter is discarded. Genuine scrolls flush exactly as
    before. Flag-independent, so it also fixes the old-layout bounce the user
    saw. DIAGNOSTIC: applyOverscroll logs "[OVERSCROLL] streak begin: d=(…) via
    drag/fling" on the first overscroll of a streak — if a tap still bounces,
    this names the magnitude + path so the residual source is unambiguous.
  - **DEVICE FEEDBACK V32.34+ (Fairphone 5 / Plasma Mobile photo + report).**
    Several findings from a real-device run:
      * ROOT CAUSE of "unified nav not working on phone": there was NO in-app
        way to enable it. unifiedNavEnabled() reads SC_UNIFIED_NAV (env var — a
        Plasma Mobile app launched from its icon never sees it) or ui/unified_nav
        QSettings (nothing wrote it). So the phone always showed the OLD TabBar,
        which explains BOTH "old bottom bar in the photo" AND "top nav still
        there / duplicated" (the pre-unified layout: bottom TabBar + per-page top
        strips, since the strip-hides are gated on the flag). FIX: added an
        in-app toggle "Unified bottom navigation" under Settings → General →
        Appearance → NAVIGATION (V32.34+ relocated here from the About-tab debug
        flags, which was a poor home for a user-facing feature) that writes
        ui/unified_nav. Needs a restart (flag is cached at startup); inline note
        + toolTip say so. THIS is almost certainly why none of the gesture work
        appeared to do anything on the phone — it wasn't running.
        ▶ V32.34+ CONFIRMED by user: they never flipped the flag on the phone
        because their build flags are hard to reprogram — the Settings toggle is
        exactly the unblock. Hardened it: lands on the About tab (unconditionally
        built, reachable in the current flag-OFF layout via the old top strip),
        persists IMMEDIATELY via setValue+sync (no Save needed — verified org/app
        names set in main.cpp:114-130 before any QSettings use, so write+read hit
        the same store), shows an inline "Saved. Restart…" note so it's
        unmistakable. Dropped the misleading markDirty() (implied a Save was
        required when it wasn't). Bar-build prereqs verified: all 5 pages added
        (MainWindow:226-230) before the bar is built (258), so pageCount()/dots
        render from first paint. NEXT: user enables toggle → restart → FIRST real
        run of unified nav on device. The bounce/"duplicated nav" reports were
        the OLD layout (feature dormant), so they can't be diagnosed until the
        unified bar is actually live — deferred, NOT guess-fixed.
      * CAMERA tiling/skew on Fairphone REAR: the explicit stride-safe YUV
        conversion only covered NV12. The rear camera likely emits NV21 or
        YUV420P, which fell through to frame.toImage() — the path that ignores
        row stride and produces the diagonal "skiing" skew + vertical tiling
        seen in the photo. Extended the explicit BT.601 path to NV12/NV21/
        YUV420P (correct per-plane bytesPerLine addressing). Added a log marker
        on any format that STILL falls through to toImage(), so if the rear is a
        4th format (e.g. YUYV packed) the next log names it exactly.
      * BOUNCE-ON-TAP + scroll-handoff SEMANTICS: deferred pending user
        clarification (asked) — the detent direction already matches "down→next,
        up→prev", but the user hints the model may be subtler and the tap-bounce
        repro is unclear. Will NOT change blind; waiting on the answers + logs.
      * NETWORK slower to connect: noted, needs the logs. The earlier paste
        showed STUN timeouts cycling through servers before a reflexive result +
        UPnP/NAT-PMP both failing → manual-forward fallback. Could be
        environmental or a regression; can't tell without a fresh log to compare
        connect timings. NOT touched this turn.
  - **FP5 SALIENCY ROI ROTATION FIX V32.34+ (4th issue from same logs).**
    ffmpeg warned "[Parsed_addroi_0] Calculated value 1248 for h is greater than
    maximum allowed value 720 - using 720 instead" — the saliency ROI box
    (addroi=185:0:483:1248) exceeded the frame. ROOT CAUSE: detectSalientRoi
    extracted a frame (ffmpeg AUTO-ROTATES per the stream's -90° display matrix,
    so the decoded frame is display-oriented 1280x720) and the saliency box is
    computed in that space, but the dimension probe read the first "WxH" in
    stderr = the CODED 720x1280, then scaled the box by coded dims while addroi
    applies it in DISPLAY dims → out-of-bounds height. Also explains the
    "portrait 9:16 source → 720x406 16:9 output": the displayed frame really is
    landscape (videoflip clockwise + display matrix), so scale=min(iw,720)
    operates on the rotated 1280-wide frame. FIX in detectSalientRoi:
      (a) anchor the dim regex to the `Video:` stream line (was grabbing any
          number, e.g. a bitrate),
      (b) parse the `rotation of N degrees` side-data and swap srcW/srcH for
          ±90/270 so we scale by DISPLAY dims,
      (c) HARD-CLAMP the final box to the frame (qBound on x/y/w/h) as a safety
          net, and drop the boost entirely if the box ≈ whole frame (useless).
    Added <utility> for std::swap. NON-BUGS investigated and intentionally left:
      * x264enc bitrate=8000 (kbit/s) vs openh264enc bitrate=8000000 (bit/s) —
        BOTH 8 Mbps; the units genuinely differ per gst element and the code
        comments document it. Not a bug.
      * VideoRecorder dtor uses the same kill()+deleteLater() shape as the old
        GstLaunchBackend, BUT its stop() already terminate()s + waitForFinished()
        + reaps synchronously before the dtor orphan path runs, so m_proc is
        NotRunning by then → no "destroyed while running". Left as-is.
      * Preview JPEG temp (/tmp/snapclone-preview-PID.jpg) IS removed on both the
        success and failure stop paths. Fine.
      * "Frame processing PAUSED (visible:false)" on navigate-away is correct
        behavior, not a leak.
  - **DEBOUNCE/COALESCE LAYER for swipe transitions (V32.34+). Rust accumulator
    BUILT & TESTED; C++ layer-separation wired.**
    USER ASK: debounce/coalesce accumulators across the stack so swipe-driven
    page transitions stay responsive but don't SPAM the underlying system
    (network/battery); may need a layer separation for robust "conceptual
    bouncing" of the UI. FINDING: every pageChanged fans out into BOTH instant UI
    sync (tab highlight) AND expensive/consequential work (MainWindow.cpp ~350:
    usage stats, AppContext page propagation that the NETWORK layer reacts to —
    peer priorities/retry pressure, ambient-blob toggle, milestone celebrations,
    edge-hint state). A fast 5-page swipe fired all that 5× in ~300ms, churning
    the network layer for pages nobody settled on. Existing debounce was ad-hoc
    per-site (UxGuide restart-the-QTimer); no shared concept.
      * RUST CRATE snapclone-debounce (pure-std, NO core dep so it stays testable
        — core pulls the edition2024 drift): CoalescingDebouncer<T> — leading+
        trailing debounce that coalesces to the LATEST value. push(value,now)→
        optional Leading edge (responsive work fires NOW); tick(now)→Settled edge
        ONCE after quiet_ms, reporting the FINAL value (never intermediates);
        max_wait_ms cap forces a settle during continuous swiping so the network
        layer isn't starved; redundant-settle suppression (swipe away+back to
        same page does no consequential work twice); flush() for shutdown.
        Caller-supplied monotonic ms → fully deterministic under test.
        **10 tests pass** incl rapid_swipes_coalesce_to_latest (5 swipes → 1
        leading + 1 settled), max_wait_forces_settle, swipe_away_and_back
        _suppresses_redundant_settle, trailing_only (no leading), flush.
      * C++ LAYER SEPARATION (wired, unverified — no Qt here): CoalescingDebouncer
        .hpp MIRRORS the Rust crate (lockstep; Rust = source of truth + tests).
        SwipeNavigator gains TWO new signals: pageAnimating (instant, every
        transition — responsive UI) + pageSettled (debounced, once — expensive
        work). pageChanged RETAINED (fires alongside pageAnimating) for
        back-compat (ComponentTestWindow, UnifiedNavBar tab-strip still follow
        instantly). All 6 page-transition emit sites funnel through one new
        notifyPageTransition(index): emits pageChanged+pageAnimating instantly,
        feeds the debouncer, and one lazy 50ms QTimer drives the trailing tick →
        pageSettled (timer self-stops when idle). MainWindow's EXPENSIVE handler
        (stats/AppContext-network/ambient/celebrations/edge-hint) MOVED from
        pageChanged → pageSettled; tab-bar highlight stays on pageChanged
        (instant). Brace-checked; namespace (SnapClone::UI) + QElapsedTimer +
        template-member-init verified.
      NOTE: Rust fully verified; C++ unverified (no Qt). The separation is the
      robust "conceptual bouncing" the user asked for: instant visual/animation
      + lightweight UI sync on the leading edge, consequential/stateful work
      coalesced to the settled page on the trailing edge. The 180ms quiet / 600ms
      cap are reasoned defaults; tune if a settle feels too eager/laggy on device.
  - **AV1 ADVANCED COMPRESSION — CRF + hierarchical reference frames + temporal
    filtering (V32.34+). Policy crate BUILT & TESTED; C++ encoder wiring written.**
    USER ASK: focus on AV1, concentrate quality on I-frames/reference frames for
    advanced compression. INSIGHT: AV1 encodes a HIERARCHICAL reference pyramid —
    I-frame + low-layer B-frames are referenced by many later frames; high-layer
    "leaf" frames are decoded once. Bits on references PROPAGATE. SVT-AV1
    concentrates quality on references AUTOMATICALLY but only well in CONSTANT-
    QUALITY (CRF) mode; the current encoder used strict TARGET-BITRATE, whose rate
    controller FLATTENS that distribution to hit the byte budget → most of the
    hierarchical advantage thrown away. Also noted the seekability TENSION: more
    I-frames = better scrub but worse compression (current code forces 1s GOP for
    seek), so this is a policy, not a constant.
      * RUST CRATE snapclone-av1policy (+ sc_av1_plan C FFI; added to workspace):
        per-tier Plan — user tiers (Standard/HighQuality) use CRF (32/26) +
        hierarchical-levels (4/5) + temporal filtering (enable-tf, cleans
        reference frames); tiny tiers (Thumb/Preview) keep target-bitrate
        (predictable size > squeezing bytes). seekable flag picks ~1s GOP (scrub)
        vs 4s (byte-efficient); GOP sized from fps so it's fps-independent.
        **9 tests pass**: user_tiers_use_constant_quality (the core idea),
        highquality_lower_crf_than_standard, seekable_short_vs_size_long_gop,
        gop_is_fps_independent, crf_in_valid_av1_range, temporal-filtering on.
      * C++ WIRING (written, unverified — no SVT-AV1/ffmpeg here): Av1Policy.hpp
        MIRRORS the Rust crate (kept in lockstep; Rust is source of truth + tests).
        ffmpeg backend (PRIMARY) now uses CRF for user tiers (-crf N -b:v 0, which
        is how libsvtav1 enables constant-quality) + -svtav1-params
        hierarchical-levels + enable-tf; bitrate branch for tiny tiers. gst
        svtav1enc FALLBACK is MORE CONSERVATIVE: keeps its target-bitrate RC
        (mixing CRF param with the bitrate property is version-dependent and
        unverifiable here) and only adds the SAFE composable wins (keyint +
        hierarchical-levels + enable-tf via parameters-string) so a wrong
        assumption can't silently break the fallback encode. kbps moved into the
        bitrate branch to avoid an unused-var warning under -Werror. Brace-checked.
      * HONEST CAVEATS: (1) C++ unverified — SVT-AV1 param NAMES
        (hierarchical-levels, enable-tf) are stable across releases and the
        ffmpeg -crf/-b:v 0 CRF idiom is well-documented, but if a param is
        rejected the encode fails and the transcoder's runtime-fallback chain
        kicks in (so a bad param degrades, doesn't crash). (2) The GAIN is real
        (CRF + hierarchy is the canonical AV1 advanced-compression path) but its
        MAGNITUDE needs on-device measurement (file size at equal VMAF). The CRF
        values (32/26) and hierarchy depths (4/5) are reasoned starting points,
        not measured optima. (3) Kept seekable=true so existing seek behavior is
        preserved; a future "delivery vs viewing" split can pass seekable=false
        for the longer-GOP byte savings.
  - **FACE COMPRESSION IN VIDEO — improved gains via ROI BIT-ALLOCATION
    (V32.34+). Rust planner BUILT & TESTED; C++ encoder wiring written.**
    USER ASK: keep working on face compression in video, improve gains.
    FINDING: the existing path applied a single ffmpeg `addroi=...:-1` that only
    BOOSTED the face (lower QP) and did NOTHING to the background — under bitrate
    rate-control that barely shrinks the file, and -1 (max) over-spends on the
    face. The FACE_BANDWIDTH doc PROMISED "compresses the background harder"; the
    code never did. So the gain was being left on the table.
      * RUST PLANNER snapclone-saliency::roi_plan (+ sc_roi_plan C FFI): turns the
        saliency box into an ORDERED bit-allocation plan — (1) background QP
        PENALTY over the whole frame (compress harder where the viewer isn't
        looking), then (2) a PADDED face QP BOOST that PROTECTS the subject from
        that global degradation (addroi-chain: later overrides earlier in the
        overlap). Net: bits move FROM background TO face → smaller file at equal
        perceived quality, the actual point of face-aware compression. Face box
        padded 30% outward (QP step lands in low-detail bg, not through the
        chin); background penalty SKIPPED when face >60% of frame; all geometry
        frame-clamped (defends the rotation-overshoot the static-box path hit).
        **8 planner tests pass** (16 total in the crate) incl
        bit_reallocation_has_opposite_signs (bits really move bg→face),
        box_overshooting_frame_is_clamped, huge_face_skips_background_penalty,
        directive_order_background_first.
      * C++ WIRING (written, unverified — no Qt here): VideoEncoderBackend ffmpeg
        path now calls sc_roi_plan and emits N addroi filters (bg penalty + face
        boost) instead of the single -1 boost; legacy single-boost (-0.30) kept
        as the !SNAPCLONE_RUST_SALIENCY fallback. EncodeSpec gains roiFrameW/H;
        VideoTranscoder records the resolved display dims (mutable
        m_lastRoiFrameW/H) so the planner clamps correctly. Guarded saliency FFI
        include added; lcEncoder category. Brace-checked; FFI signature +
        arg-count verified.
      * DEFERRED: per-frame ROI for moving subjects (needs the in-process libav*
        rewrite, already filed). The plan improves the common stationary-subject
        case without that larger rewrite. Tunables in RoiTuning if defaults need
        adjusting after on-device measurement.
      NOTE: Rust fully verified; C++ unverified. The gain is REAL but its
      MAGNITUDE needs on-device measurement (file size at equal VMAF/perceived
      quality) — the defaults (-30 face / +25 bg / 30% pad) are reasoned starting
      points, not measured optima.
  - **PiP ROOT CAUSE + WEBCAM-ARRAY/SCREENSHARE DESIGN + simultaneity model
    (V32.34+). Diagnosis + conservative C++ fix shipped; Rust model BUILT &
    TESTED; platform glue deferred.**
    USER ASK: understand why Picture-in-Picture is still broken + how to add
    desktop webcam arrays and PipeWire screenshare.
      * ROOT CAUSE (honest): the PiP *code* (startPip/onPipFramePreview/overlay/
        retry/diagnostics in CameraWidget.cpp) is SOUND. The breakage is a
        hardware constraint — PiP runs TWO pipewiresrc at once (rear+front), but
        the FP5 (single-ISP mobile stack) shares one pipeline across front+rear
        and CANNOT do both; the 2nd negotiation fails ("Failed to build output
        pipeline" — matches the camera-bridge logs on record). backendHasFront-
        AndRear() only checked both cameras EXIST, never that they can run
        SIMULTANEOUSLY → PiP was offered on hardware that can't deliver it, then
        silently showed a frozen black tile (retry-once can't fix a HARD limit).
      * UNIFYING INSIGHT: GstLaunchBackend already targets ANY pipewire node via
        `pipewiresrc target-object=<node>`. A physical cam, USB webcam, virtual-
        cam, and a portal ScreenCast stream are ALL just nodes. So don't add
        "screenshare" + "webcam arrays" as 2 features — generalize what a SOURCE
        is + model which sources can run together. Desktop UVC webcams are
        independent → simultaneous works (PiP/multi-cam correct there); the SAME
        path that "fails" on mobile is right on desktop once the concurrency
        check stops blocking it. Screenshare = xdg-desktop-portal ScreenCast
        DBus handshake → returns a pipewire node → feed the EXISTING backend.
      * DESIGN DOC docs/PIP_AND_SCREENSHARE_DESIGN.md (source kinds table,
        concurrent-capture-group model, portal handshake steps, privacy notes:
        screenshare frames go through the same E2E crypto, portal dialog is the
        consent boundary, NO screenshare in the thin server binary).
      * RUST CRATE rust/snapclone-videosrc/ (pure-std, added to workspace): the
        simultaneity check PiP was missing. SourceKind (PhysicalCamera/UsbWebcam/
        VirtualCamera/ScreenCast), CaptureGroup (Option<u32>; shared exclusive
        group = can't coexist, None = freely concurrent), can_run_simultaneously
        → Ok or Conflict{user_message}, compatible_secondaries (what to OFFER as
        a 2nd source). **9 tests pass** incl fp5_front_and_rear_conflict (the
        headline bug as a test), independent_usb_webcams_run_together, screenshare
        _plus_camera_ok, compatible_secondaries_excludes_isp_sibling.
      * C++ FIX SHIPPED (conservative gate until the per-source model is wired):
        CameraPolicy.simultaneousCaptureSupported (key camera/policy/
        simultaneousCapture, DEFAULT FALSE) + load/save. backendHasFrontAndRear()
        now requires it (so PiP isn't offered on FP5); onDeviceSelected PiP
        branch hard-guards + falls back to rear with a clear log instead of a
        black tile. Settings → Camera advanced: "Allow simultaneous cameras
        (Picture-in-Picture)" toggle. Brace-checked; Theme/API verified.
      * DEFERRED (no Qt/DBus/PipeWire here): pw-dump classification (cam vs UVC
        vs screencast), the portal ScreenCast handshake, multi-source compositor
        UI, wiring concurrency-groups through CameraRegistry. Spec'd precisely in
        the design doc §4.
      NOTE: C++ unverified (no Qt here); Rust model fully verified. The fix makes
      PiP HONEST (don't offer what the device can't do, fail with a reason) — it
      does NOT make the FP5 do simultaneous dual-camera, which is physically
      impossible on a shared ISP. PiP becomes correct on desktop/USB-webcam
      hardware once simultaneousCapture is enabled.
  - **GROUP CHAT + DISTRIBUTED MEDIA + CLIENT/SERVER SPLIT — DESIGN + group
    crypto crate (V32.34+). Design settled; foundational crypto BUILT & TESTED.**
    USER ASK: group chat over the P2P+DHT substrate; multi-recipient DHT mailbox
    + distributed group photo/video sync; and a two-binary split (full mobile
    client + thin LAN server browsers connect to for text/landmarks/public
    ephemeral groups, good for "down the street" FCC-modem / ad-hoc hosting).
    Flagged (correctly) as crypto-heavy → careful consideration first.
      * DESIGN DOC `docs/GROUP_AND_SERVER_DESIGN.md` (settles interlocking
        decisions before code): sender-key scheme (one content key per message,
        media encrypted ONCE regardless of group size; only small per-recipient
        key-distribution messages fan out N-way); membership EPOCHS that re-key
        on add/remove (add → post-compromise security, remove → forward secrecy,
        O(N) re-key accepted vs MLS/TreeKEM O(log N) which is deferred); DHT
        group-feed bucket SHA-256("group-feed:"+gid+":"+epoch) polled by all
        members (no content fan-out) vs per-recipient bucket for key dist; SEALED
        group entry closes S-19 (DHT sees only routing bucket + opaque blob +
        Ed25519 sig, never social graph/size); content-addressed media
        (hash=SHA-256(ct)) with ANY member who fetched becoming a re-seeder →
        swarm, ephemeral by TTL; §3 crypto checklist (reuse-don't-reinvent,
        versioned domain-sep labels, random nonce, no key reuse across roles,
        verify-before-decrypt, removed-member-safety-is-a-test). TWO-BINARY:
        confirmed feasible — core/network/storage/crypto are separate static
        libs and neither network nor core includes ui/, so a QCoreApplication
        `snapclone-server` linking everything-but-ui is a new executable + small
        main, no source moves. Server is a constrained participant + browser WS/
        HTTP gateway, ONLY fully participates in PUBLIC EPHEMERAL groups (public
        gid, open keys = integrity not confidentiality, TTL-bound, text+landmark
        view only, no camera); cannot decrypt private groups (never holds sender
        keys) — safe for untrusted "down the street" hosting.
      * RUST CRATE `rust/snapclone-group/` (added to workspace; the riskiest
        part, built FIRST + fully tested): labels.rs (centralized versioned HKDF
        domain-sep), ratchet.rs (one-way sender-key HKDF chain, zeroized,
        forward-secret), seal.rs (per-recipient sender-key seal over X25519→HKDF→
        AES-256-GCM + AEAD content enc/dec), group.rs (GroupId grp-<64hex>, feed
        bucket, GroupState with add/remove epoch re-key flow). VETTED primitives
        only (hkdf/aes-gcm/ed25519/x25519/sha2/zeroize — all already in lockfile,
        NO new dep surface, NO homegrown crypto). **16 tests pass**, including
        the two headline safety properties proven as tests:
        removed_member_cannot_read_new_epoch + added_member_cannot_read_prior_
        epoch, plus within-epoch forward secrecy, tamper/wrong-key/wrong-
        recipient fail-closed.
      * NEXT STEPS (design §5, untestable here → deferred): group mailbox buckets
        + sealed GroupMailboxEntry in snapclone-mailbox (testable vs micro-DHT);
        sc_group_* FFI; C++ group session + UI (behind flag); snapclone-server
        binary + browser gateway. The crypto FOUNDATION is proven; everything
        above it builds on tested math.
      * UPDATE (continuation): the sealed GROUP ENTRY + an END-TO-END test
        through the micro-DHT are now DONE (the §5 "next verifiable step").
        snapclone-group::entry — a sealed+signed GroupMailboxEntry: hand-rolled
        deterministic binary framing (no serde, avoids the getrandom/edition2024
        dep drift that breaks crypto/mailbox in this sandbox), body
        {epoch,index,kind(Text/MediaRef/Control),payload} encrypted with the
        per-message ratchet key, wrapped with an Ed25519 sig over magic+sender_id
        +sealed+pubkey. verify_and_open authenticates BEFORE decrypting (verify-
        before-trust); peek_sender_id reads the cleartext routing id without a
        key. 8 entry tests: tampered-ciphertext→sig fail, forged-sig→fail,
        valid-sig+wrong-key→decrypt fail (no plaintext leak), truncated/bad-magic
        rejected. INTEGRATION TEST (tests/dht_round_trip.rs, microdht as
        dev-dep): full stack — Alice creates group + adds Bob + seals her sender
        key to him → composes → seal+sign → PUT to feed_bucket(group,epoch) on a
        live micro-DHT node → Bob GETs the opaque bytes, ratchets Alice's chain,
        verify+opens → reads plaintext; an eavesdropper scraping the same bucket
        gets only ciphertext. **25 group tests pass total** (24 unit + 1
        integration). The DHT only ever saw the bucket key + opaque signed/sealed
        bytes. STILL DEFERRED: C++ group session + UI, snapclone-server binary
        (untestable here). FFI now DONE — see next bullet.
      * UPDATE (continuation 2): sc_group_* C FFI is DONE and genuinely VERIFIED
        (it's a Rust change, testable here unlike the Qt glue). snapclone-group::
        ffi — STATELESS C ABI mirroring snapclone-crypto's pattern (C++ keeps
        GroupState as opaque bytes; only calls Rust for the two ops it can't
        safely reimplement). sc_group_seal_and_sign / sc_group_verify_and_open
        (verify-before-decrypt) / sc_group_peek_sender_id. GroupFfiError enum
        (Ok/NullPointer/BufferTooSmall/BadKind/OpenFailed/SealFailed/Internal).
        Caller-provides-buffer pattern for variable-length wire/payload (call
        with cap=0 to size, retry); OsRng internally (rand_core getrandom feature
        → working getrandom 0.2.15, NOT the edition2024 0.4.2 — same trick as
        mailbox); every entry point catch_unwind so a panic becomes Internal not
        UB across the ABI. Crate now crate-type=["staticlib","rlib"]. **5 FFI
        tests pass** exercising the raw-pointer/caller-buffer path C++ uses incl
        the BufferTooSmall retry + security rejections (wrong key→OpenFailed,
        null→NullPointer, bad kind→BadKind). **34 group tests total** (29 unit
        incl 5 FFI + 1 integration; the 5-test count shown when filtered). VERIFIED
        BEYOND THE USUAL: libsnapclone_group.a builds in BOTH debug and release
        (release = what CMake builds), and include/group/snapclone_group_ffi.h
        passes gcc -fsyntax-only as BOTH C and C++. CMake: top-level
        SNAPCLONE_RUST_GROUP option + build block added (mirrors saliency).
        target_link_libraries deliberately NOT wired yet — documented in
        src/ui/CMakeLists.txt to turn on in the SAME change as the first C++
        caller (no dead link to a target with no group symbols). STILL DEFERRED:
        C++ group session + UI, snapclone-server binary.
      * UPDATE (continuation 3): EPHEMERAL GROUP ORDERING via a content-addressed
        Merkle DAG — DONE + TESTED. User idea: keep groups ephemeral, double-check
        distribution with a git-like store of snap hashes for smooth ordered
        distribution across a distributed system; "only need the basic
        functionality of that tree type for out-of-order messaging + conflict
        resolution." DECISION: take the part of git that solves this (the Merkle
        DAG — objects name parents by hash, graph = causal history, replicas diff
        by comparing tips) and NONE of the rest (no files/packs/refs/working-tree/
        merge). Embedding real git = wrong tool (heavyweight, disk/checkout-
        oriented, unwanted merge model); we need the data structure, not the
        program. Doc: docs/GROUP_DAG_ORDERING.md (incl why-not-CRDT/vector-clocks/
        embedded-DB).
        RUST MODULE snapclone-group::dag (no new deps — reuses sha2/hex): DagNode
        {author, epoch, lamport, parents:[NodeId], content_hash} where NodeId =
        sha256(canonical_bytes) (content-addressed, tamper-evident like a git
        object; parents sorted+deduped so id is stable). SnapDag: author_node
        (extends current tips, bumps Lamport), insert (ANY order — buffers orphans
        whose parents haven't arrived + indexes missing parents to fetch, cascades
        admit when parents land), missing_for(peer_tips) (causal-closure diff →
        send ONLY the delta, git have/want in miniature), total_order (sort by
        (lamport,author,id) — deterministic, arrival-independent convergence),
        tips/pending_fetches, Lamport invariant validation (forged clocks
        rejected). **11 tests pass** incl replicas_converge_to_same_total_order
        (same nodes, different arrival order → identical order),
        out_of_order_insert_buffers_then_admits (cascade), missing_for_sends_only
        _the_delta, forged_lamport_clock_rejected, merge_two_branches (concurrent
        branches → 2 tips → join → 1 tip). **40 group unit tests total** (+1
        integration). LAYERING: DHT value layer → sealed+signed entry
        (confidentiality+authenticity, already built) → DAG node (ordering),
        each independently tested; a DagNode serializes into a GroupEntryBody so
        ordering metadata is itself encrypted+signed on the wire (S-19 stays
        closed). DEFERRED (next Rust-verifiable rung): DagNode↔GroupEntryBody
        serialization + sc_group_dag_* FFI; then the sync loop (announce tips,
        fetch deltas) + C++ session — all on top of the tested core.
  - **MICRO-DHT FOR BETA TESTING V32.34+ (Rust crate + C++ shim + Settings
    submenu — Rust side BUILT & TESTED here; C++ side written, untested).**
    USER REQUEST: a micro-DHT in Rust to test against the real DHT, plus a
    "beta testers" settings submenu. DECISION: a LOCAL value-layer stand-in
    reproducing the app-visible DHT semantics (sc-ID keyed get/put, opaque
    encrypted values, TTL expiry, multi-value-per-key, re-announce dedup) over a
    trivial UDP TEXT protocol — NOT OpenDHT-wire-compatible (no Kademlia/routing/
    node-IDs). Pure-stdlib Rust so it builds offline + runs as a standalone test
    peer; app swaps real↔micro via a settings toggle through the same abstraction.
      * RUST CRATE `rust/snapclone-microdht/` (added to workspace members):
        store.rs (TTL HashMap<key,Vec<Entry>>, re-announce dedup+refresh, lazy +
        background sweep, counters), protocol.rs (text UDP: PING/PUT/GET/STATS →
        PONG/OK/VALUES/STATS/ERR, hex enc/dec), node.rs (UDP server, 250ms read-
        timeout poll loop, background sweeper, MAX_TTL_SECS=3600 cap), client.rs
        (blocking client), main.rs (`sc-microdht` binary, --bind/--quiet/--help).
        NO external deps. BUILDS in ~19s; **all 19 tests pass** incl end-to-end
        over real UDP + client-vs-live-node. Binary smoke-tested over real UDP:
        PING/PUT/GET/STATS/ERR all behave. (Ctrl-C is best-effort — no signal
        crate; relies on read-timeout loop + OS terminate.)
      * C++ SHIM `Network::MicroDhtClient` (include/network/MicroDhtClient.hpp +
        src/network/MicroDhtClient.cpp, added to src/network/CMakeLists.txt +
        AUTOMOC list): async QUdpSocket client speaking the same protocol, same
        lowercase-hex (validates strictly like the Rust decoder). Signals
        pong/putAcknowledged/valuesReceived/statsReceived/nodeError/
        requestTimedOut; per-verb timeout timers; literal-IP only (no DNS).
      * SETTINGS SUBMENU: buildBetaTestersSection added to the About tab
        (SettingsPage). Toggle beta/micro_dht_enabled, host:port field
        beta/micro_dht_host (default 127.0.0.1:4544), "Test connection" button
        that PINGs and shows live status. Brace-checked; Theme accessors
        (TEXT_SECONDARY()/FONT_SM) + signal signatures verified by hand.
      * INTEGRATION BOUNDARY (deliberately deferred): the toggle records intent
        but the LIVE routing is NOT yet wired — making DhtDiscovery's
        announceAddress/announcePresence/lookupPeer/publishConnectionRequest
        (JamiAdapter_dht_impl.cpp) consult the flag and use MicroDhtClient is a
        small reviewable change left out because it touches the load-bearing
        real-DHT path that can't be compiled/tested here. Documented in the
        header + crate README so the follow-up is unambiguous.
      NOTE: C++ side unverified (no Qt headers here); Rust side fully verified.
  - **COMBOBOX WHEEL GUARD + STRIDE-ALIGN TILING FOLLOW-UP V32.34+ (closing two
    things I'd flagged-and-deferred).**
      * COMBOBOX WHEEL GUARD: the 6 QComboBoxes in Settings (rotation, audio-comp
        level, camera-facing override, discovery backend, relay preset, crypto
        backend) had the same wheel-changes-selection default as the spinboxes —
        scrolling the page past one silently changed the selection. Applied the
        existing installWheelGuard() to all 6 (generalized its comment). Guard
        only consumes wheel on the COLLAPSED, unfocused combo; an OPEN dropdown
        is a separate popup widget the guard never sees, so list-scrolling still
        works. 4 spinboxes + 6 combos = 10 guarded value widgets.
      * STRIDE-ALIGN TILING (likely the actual root cause): bumped
        CameraPolicy.strideAlignPx default 4 → 16. The old value rounded width to
        a multiple of 4 (16-byte stride alignment), but gldownload (GPU path) and
        libcamera/v4l2 sources commonly align rows to 64 BYTES. width=1284 →
        tight row 5136, a 64-byte-aligner pads to 5184, reader assumes 5136 →
        diagonal walk → tiled framebuffer. strideAlignPx=16 forces width to a
        multiple of 16 so width*4 is 64-byte aligned → tight==padded for the
        common case. This is more likely to actually fix the tiling than the
        prior speculative source-side videoconvert (kept as belt-and-suspenders).
      * BLOAT DIAGNOSTIC: the buffer-bloat detector now INFERS the likely real
        frame size by testing 16/32/64/128/256-byte row alignments and reporting
        which divides the buffer cleanly — turning "something's off" into "the
        real stride is probably N", so a persisting tile is one log line from
        diagnosis. (It still clears the buffer to recover; the clear-loop was
        non-recovering for padded stride, but the width-align fix removes the
        padding so the loop shouldn't engage.)
      NOTE: all unverified on device. The decisive next datapoint if tiling
      persists is the "Negotiated SINK caps: ... width=N height=N" line plus any
      "Frame buffer bloat ... Stride candidates:" line.
  - **TOUCH DRAG → UNIFIED-NAV PARITY V32.34+ (integrate touch into the same
    detent/handoff system as the wheel, respecting the mechanism differences).**
    Audit finding: touch ALREADY fed the shared detent — applyOverscroll()
    (touch drag + fling, never the wheel) calls evaluateVerticalDetent(), which
    is where the takeover-lock + page-spill + direction logic lives, so touch
    already inherited those. The genuine gaps vs the improved wheel path:
      * LOCK-GATING PARITY: feedWheelVerticalDetent zeroes its accumulator while
        scrollTakeoverLocked(), but applyOverscroll did NOT — so a continued
        finger drag right after a detent fire kept piling into m_overscrollRawY
        and re-fired the instant the lock expired (the same runaway the lock was
        built to stop, via touch). FIX: applyOverscroll now zeroes the vertical
        accumulator + settles the viewport and skips the evaluator while locked
        (under unified nav only; horizontal overscroll + rubber-band preserved).
      * RATE-NORM is correctly NOT applied to touch — wheel needs it (events at
        refresh rate), but touch deltas are absolute physical pixels, so the
        220px detent threshold is a real finger-travel distance. Leaving touch
        un-normalized is the right mechanism difference, not a gap.
      * DIRECTION: verified touch is already CONSISTENT with the wheel (both
        pass the raw signed delta to the same evaluator; at the bottom both map
        the continued pull the same way), so no sign flip was needed — my first
        instinct that touch was "backwards" was wrong once I traced the wheel's
        own wheelAtVerticalLimit sign convention.
      * INVERT TOGGLE PARITY: added ui/scroll_detent_invert_touch +
        m_detentInvertTouch + a third "Reverse scroll-to-navigate (touch drag)"
        toggle in Settings → Appearance → NAVIGATION. Applied ONLY to the value
        handed to evaluateVerticalDetent (a flipped copy) so the visual
        rubber-band still tracks the finger; the wheel path is untouched because
        applyOverscroll is never reached by the wheel. Default false (touch has
        a natural physical mapping).
      NOTE: unverified on device — needs a touch check that drag-past-edge
      advances sub-tab/page once (not a runaway) and that the lock holds between
      fires. The [OVERSCROLL] streak begin: ... via drag/fling log already
      distinguishes the touch path for debugging.
  - **TILED PREVIEW FRAMEBUFFER V32.34+ (persistent — my earlier NV12/NV21/
    YUV420P stride fix was for the WRONG path).** Correction: that fix was in
    QtCameraBackend (Qt QCamera path). On FP5 the live PREVIEW comes through
    GstLaunchBackend: gst-launch -> fdsink -> a raw pipe the app slices into
    QImages. The tiling (one frame rendered as several shrunken copies) is the
    classic signature of slicing the pipe at the WRONG byte boundary / wrong
    stride. ROOT CAUSES + FIXES:
      * The reader assumed the pipe bytes were tightly packed at the REQUESTED
        m_frameWidth/m_frameHeight (slice = w*h*4, QImage bytesPerLine = w*4).
        It never read the ACTUAL negotiated dims. FIX: extended the existing
        fdsink sink-caps parser (which only pulled `format` for the R/B swap) to
        also extract width/height; when they differ from the request we adopt
        them, recompute m_frameBytes, set m_sinkWidth/Height/Stride, and clear
        m_buf to re-sync on a true frame boundary. The QImage now uses
        m_sinkStride / m_sinkWidth / m_sinkHeight when known.
      * GPU path was the likely real source: gldownload transfers GL memory to
        sysmem, but GL textures align to 8/16/32/64 px, so the downloaded ROW
        STRIDE is padded beyond width*4 and total size beyond w*h*4 — and the
        old sysmem capsfilter pinned format but NOT size ("NO size" comment).
        FIX: after gldownload, force `videoconvert ! video/x-raw,format=BGRx,
        width=%1,height=%2` so the bytes hitting fdsink are deterministic tight-
        packed w*h*4 (for BGRx, GST_ROUND_UP_4(w*4)==w*4). One cheap pass over
        the small preview frame. CPU path already pinned exact dims so it was
        fine; this makes the GPU path equally deterministic.
      * `-v` (which emits the negotiated caps line on stderr) was gated on
        policy.logFirstFrameInfo. The fix DEPENDS on that caps line, so `-v` is
        now UNCONDITIONAL (stderr-only, log-category-gated, no user cost).
      New members: m_sinkWidth/m_sinkHeight/m_sinkStride (reset each start).
      NOTE: unverified on device — the two fixes are complementary (source-side
      tight-pack + reader-side re-sync), but I CANNOT compile/run here, so this
      needs a device check. If tiling persists, the next data point is the
      "Negotiated SINK caps: ... width=N height=N (requested WxH)" log line the
      parser now prints — it will show whether dims mismatch or the stride
      assumption is still wrong.
  - **FP5 CAMERA LOG TRIPLE-FIX V32.34+ (from three device logs; all three
    issues confirmed and addressed):**
      * RE-ENUMERATION STORM (battery/thermal + log flood). available() lands in
        libcameraFallback() on FP5 (virtual-cam bridge absent), and a ~250ms
        caller made it re-spawn `cam --list` + re-parse + emit ~6 qCInfo lines
        4×/sec, continuously, while the camera page is open. The raw pw-dump had
        a 500ms cache but the CLASSIFICATION did not. FIX: added a classified-
        result cache (m_classifiedCache/m_classifiedTimer/m_classifiedValid)
        keyed on the same kPwDumpCacheMs window — hot path returns the cached
        QList<CameraInfo> with no spawn/parse/log. Verbose logs now gated behind
        a topology-signature change (m_lastClassifiedSig = concat of node names)
        so they fire only when the device set actually changes. invalidateCache()
        clears the new cache too so override edits still take effect.
      * gst-svtav1enc TRANSCODE FAILS ON FP5 (matroskademux not-negotiated -4),
        while ffmpeg-libsvtav1 succeeds on the same .mkv. Before, a runtime
        encode failure went straight to "send raw .mkv". FIX: RUNTIME FALLBACK in
        VideoTranscoder — store the active EncodeSpec + a m_failedBackends list;
        on a non-zero/empty-output exit, mark that backend failed and retry with
        findAvailableBackendExcluding() (next backend in priority order:
        ffmpeg->gst-svtav1->vp9->vp8) before emitting transcodeFailed. State
        resets on success or when no backends remain. NOTE: did NOT fix the gst
        pipeline caps negotiation itself — the fallback makes it moot, and the
        ffmpeg path is already preferred when its availability probe (which can
        transiently time out under load, explaining why one session picked gst)
        succeeds. HARDENING (same turn): (a) cancel() now sets m_cancelled so the
        finished()-handler treats the resulting non-zero exit as a user cancel,
        NOT a runtime failure — otherwise the fallback would restart the
        transcode on the next backend right after the user cancelled. (b) the
        m_proc allocation site defensively cleans any pre-existing process
        (disconnect+kill+deleteLater) before new'ing, so a retry can't orphan the
        old QProcess. (c) fallback state (m_haveActiveSpec/m_failedBackends/
        m_cancelled) resets on every terminal outcome (success / hard-fail /
        cancel) and on a fresh start (detected by input+output path differing
        from the in-flight spec), so retries chain forward and new transcodes
        start clean.
      * QProcess "Destroyed while process still running" on every camera stop().
        Normal-path stop() did kill() (async) + bare deleteLater(), so ~QProcess
        ran before the OS reaped gst-launch -> cosmetic warning. FIX: orphan the
        process, connect its own finished()->deleteLater() (self-connection, safe
        after disconnect(this)), terminate() gracefully (the -e flag flushes
        EOS), and a 3s singleShot(ctx=proc) kill+delete guard for a wedged child.
        Delete now happens only after reap -> no warning, still no blocking
        waitForFinished() on the swipe-hideEvent call stack. Shutdown path
        unchanged (SIGKILL + synchronous reap + direct delete).
  - **PORT-OVERWRITE (real cause) + PER-SOURCE SCROLL INVERT V32.34+ (device log
    10:43 confirmed lock + rate-norm WORK; runaway gone, one detent fired then
    stopped). Two remaining bugs fixed, two issues deferred:**
      * WEBSOCKET PORT NUMBERS OVERWRITTEN: ROOT CAUSE = QSpinBox's DEFAULT
        wheel-changes-value behavior. Scrolling the Network page past m_wsPort /
        m_gossipPort silently incremented/decremented the port — NOT a timer
        loop (last turn's hypothesis was wrong). FIX: WheelGuard event filter +
        installWheelGuard() applied to all 4 spinboxes (m_wsPort, m_gossipPort,
        the addSpinRow camera spin, the camera-policy spin). It consumes Wheel
        events UNLESS the box has focus; the page still scrolls because
        SwipeNavigator's APP-LEVEL filter already saw the same event first.
        setFocusPolicy(StrongFocus) so the box doesn't grab focus on hover.
        TODO: QComboBox has the same wheel-changes-selection default — guard
        those too (rotation/format dropdowns) in a follow-up.
      * MOUSE STILL REVERSED after the trackpad invert: a single flag can't
        serve both because natural scroll usually inverts the TRACKPAD but not
        the MOUSE WHEEL. FIX: invert the sign PER-SOURCE at processWheelEvent
        (fromTrackpad = !pixelDelta.isNull()), driven by TWO settings —
        ui/scroll_detent_invert (trackpad) + ui/scroll_detent_invert_mouse
        (mouse), both read at construction. Removed the now-redundant invert in
        the detent direction calc (sign is normalized upstream — would have
        double-flipped). Two Settings → Appearance → NAVIGATION toggles, labelled
        (trackpad) / (mouse wheel).
      * DEFERRED — SCROLL DISTANCE TOO SHORT + NO SMOOTH SCROLL: the child
        QScrollArea uses Qt's default small wheel step (~3 lines) and discrete
        jumps. This is in the scroll areas (ScrollAreas.cpp), not the navigator;
        the navigator returns false for non-at-limit vertical wheel and lets the
        child scroll itself. Needs a focused change: larger single-step +
        animated (smooth) scrolling on the scroll areas. NOT done this turn to
        keep the surface small.
  - **COMPILE FIXES V32.34+ (caught by user's real compiler; brace-check can't
    catch these classes).** Two errors from the scroll-lock turn:
      * qBound<qint64>(1400, ms, 3000) was AMBIGUOUS — the 1400/3000 literals are
        int, ms is qint64, so the explicit <qint64> couldn't pick a candidate.
        Fixed with Q_INT64_C(1400)/Q_INT64_C(3000) so all three args are qint64.
      * "lock=%d" formatted kVDetentRawThreshold which is a qreal (220.0), not
        int → -Wformat. Changed that specifier to %.0f. Swept all other %d/%lld
        in the file: remaining %d take int args, all %lld are static_cast<long
        long>. Reminder: this env has g++ but NO Qt6 headers, so format-string
        and template-arg-ambiguity errors only surface on the user's build —
        brace-check + manual type sweep is the mitigation, not a compile here.
  - **REFRESH-RATE NORMALIZATION + INVERT TOGGLE + INPUT-OVERWRITE V32.34+
    (clarified user intent: smoother/non-frame-locked scroll, don't overwrite
    user-set fields at monitor pace, reverse-scroll trackpad).**
      * FRAME-RATE COUPLING (root of "fixed unless 60Hz / runs at monitor pace"):
        wheel/trackpad events arrive at the display refresh rate, so a 240Hz
        panel delivered ~4× the events/sec and — since each event's dy was
        accumulated equally — the detent built up & fired ~4× faster than on
        60Hz (the rawY=860 burst). FIX: feedWheelVerticalDetent now CLAMPS the
        accumulation to a wall-clock budget (kDetentRawPerMs=0.75 raw/ms via
        m_lastDetentFeedMs elapsed): a same-frame flurry can contribute at most
        budget=0.75*dtMs, so 240Hz and 60Hz reach the 220 threshold in the same
        ~290ms of sustained scroll. The fling/spring-back paths were already
        time-based (dt*velocity), so only the detent accumulator needed it.
      * REVERSE-SCROLL TRACKPAD: investigated the wheel-vs-trackpad filtering
        hypothesis. Qt already separates them (trackpad→pixelDelta,
        mouse→angleDelta via the isNull() branch) and wheelAtVerticalLimit +
        the detent direction BOTH key off the same vPx sign, so there's no
        edge/direction INCONSISTENCY bug — but the felt direction is wrong for
        natural-scroll users. Added Settings → Appearance → NAVIGATION toggle
        "Reverse scroll-to-navigate direction" (writes ui/scroll_detent_invert,
        read once at construction into m_detentInvert; restart note). User
        confirmed they use reverse scroll → they should enable this.
      * INPUT FIELDS OVERWRITTEN AT REFRESH PACE: confirmed by inspection it is
        NOT a Settings timer/loop writing to spinboxes/edits (fields set once at
        build; the only refresh timer touches thermal/battery LABELS). Strong
        hypothesis: it was a SYMPTOM of the runaway detent — rapid sub-tab/page
        switches under the user at 240Hz made edited fields appear to reset as
        the view changed. The LOCK (stops repeat fires) + rate normalization
        (decouples from refresh) address the mechanism. FLAGGED: if it persists
        after these, it's a separate bug needing a concrete repro (which field,
        which submenu). NOT claiming fixed outright — addressed indirectly.
      NEEDS DEVICE: confirm scroll feels smooth/refresh-independent, the invert
      toggle fixes direction, and fields no longer reset.
  - **SCROLL-TAKEOVER LOCK + DIRECTION SETTING + PAGE SPILL V32.34+ (from device
    log 09:33).** Log confirmed the gate fix worked ([WHEEL] fires constantly,
    [HANDOFF] scroll→sub-tab succeeds) but surfaced three issues:
      * RUNAWAY: one sustained smooth-scroll fired the detent ~1/sec and a single
        burst hit rawY=860 (4× the 220 threshold) — dozens of v=20 events in a
        few ms all accumulating. FIX: scroll-takeover LOCK. After any scroll-
        driven takeover (sub-tab detent OR page change) further takeover is
        blocked until now + scrollTakeoverLockMs(), where that =
        800 + max(600, 0.01*pages + 0.05*subPagesPerMin*subTabs), clamped
        [1400,3000]ms (user's formula). recentTakeovers (60s window) feeds the
        rate term. Lock gates BOTH the detent fire AND feedWheelVerticalDetent's
        accumulation (so rawY can't keep building during the lock — kills the
        860 runaway at the source), and is engaged on page-commit too so a
        still-moving scroll can't immediately hijack the new page.
      * DIRECTION inverted vs user's expectation: depends on natural-vs-
        traditional scroll, which Qt doesn't expose. Rather than hard-code (and
        flip-flop), added ui/scroll_detent_invert QSettings (read once at
        construction into m_detentInvert). Base: rawY>0→NEXT, rawY<0→PREV;
        invert flips. ASKED user which scroll mode they use to set the default;
        TODO: wire a Settings → Appearance toggle for it once confirmed.
      * "PAGES ONLY VIA SWIPE": scroll past the LAST sub-tab did nothing
        (advanceSubTab returns false at the boundary and the caller ignored it).
        FIX: PAGE SPILL — when advanceSubTab returns false at an edge, commit a
        page change in the scroll direction (goToPage(m_currentIndex+dir)). This
        is the "push past a sub page to the next page" model the user described.
        New log line "[HANDOFF] scroll→page: at sub-tab edge…". REFINEMENT TODO:
        backward spill should land on the new page's LAST sub-tab (currently
        resetSubTab=false preserves target's last state) for continuous feel.
      NEEDS DEVICE re-test: lock feel, direction default, page-spill.
  - **WHEEL EVENTS BLOCKED BY REGISTRATION GATE V32.34+ (the ACTUAL laptop root
    cause — the earlier wheel-detent fix was necessary but not sufficient).**
    Device retest: scroll→sub-tab AND two-finger horizontal STILL didn't work on
    laptop even after the processWheelEvent vertical-detent fix below. Reason
    found by tracing the event path: SwipeNavigator's app-level eventFilter has a
    REGISTRATION GATE — `if (!m_filteredWidgets.contains(obj)) return` — that
    bails BEFORE the wheel handler (line ~2923). m_filteredWidgets is populated
    by childEvent on the navigator's DIRECT children + their descendants AT
    ATTACH TIME (installFilterRecursive). Widgets created/added INSIDE a page
    after it was attached (the contact list, camera roll, scroll areas, basically
    every scrollable surface) are never registered, so a wheel over them bailed
    at the gate and never reached processWheelEvent → neither the vertical detent
    nor the horizontal two-finger page-nav could ever fire on a laptop. FIX: let
    QEvent::Wheel BYPASS the registration gate (the wheel handler does its own
    cursor-targeting via wheelAtVerticalLimit / widgetAt, so it doesn't need the
    widget pre-registered). Touch still respects the gate (its gesture lifecycle
    relies on the registered set). DIAGNOSTIC added: "[WHEEL] v=… h=… subTabbed=…
    atVLimit=…" logs once per wheel-begin under the flag — if it now appears
    while scrolling, the wheel is reaching the handler (gate fixed); atVLimit
    tells whether the scroll area reported its edge. NEEDS DEVICE re-test.
  - **WHEEL/TRACKPAD VERTICAL DETENT FIX V32.34+ (the laptop bug — root cause).**
    Device report: with the flag ON, "scroll to end → friction → page/sub-tab"
    did NOT fire on laptop. ROOT CAUSE found: processWheelEvent bailed on ANY
    vertical-dominant wheel (`if (hPx < vPx*ratio) return false`) — it was built
    only for horizontal page-swipe-by-trackpad. So vertical wheel NEVER reached
    applyOverscroll → evaluateVerticalDetent; the entire vertical detent path was
    unreachable via wheel/trackpad (it only ever worked via touch overscroll).
    FIX: on vertical-dominant wheel, if the scroll area under the cursor is at
    its vertical limit in the scroll direction (or there's no v-scrollable area
    there), feed the overflow to the detent. A SECOND blocker: applyOverscroll
    early-returns without a viewport target (none on the wheel path), so it'd
    have silently no-op'd — added feedWheelVerticalDetent(dy) which accumulates
    into m_overscrollRawY and calls evaluateVerticalDetent DIRECTLY, bypassing
    the viewport requirement. Streak reset added on detent-window lapse so wheel
    accumulation doesn't carry stale progress (no spring-back to zero it on the
    wheel path). Gated unifiedNavEnabled(); plain build's vertical wheel still
    goes untouched to the child. The "[HANDOFF] scroll→sub-tab" log will now
    actually appear on laptop. NEEDS DEVICE re-test — this is the fix for the
    reported failure.
  - **UNDERLINE MORPH + MATERIAL BARRIER UPGRADE V32.34+.** (1) MORPH: the bar's
    active-segment underline now ANIMATES between segments on a tap/detent
    sub-tab change (200ms OutCubic QVariantAnimation) instead of snapping —
    interpolates the underline span from where it visually sits to the new
    segment, handles rapid re-taps from the mid-flight position, yields to the
    drag preview when a drag is active. Driven by paintEvent reading the
    interpolated span (NO setStyleSheet — same teardown-safe discipline as the
    badges/drag preview). (2) BARRIER: the detent edge-glow got more material —
    a smoothstep-eased opacity ramp (gentle then firm, so the boundary "resists"
    more toward commit) plus a crisp brighter RIDGE line right at the edge over
    the diffuse glow (like Material's scroll-edge highlight), thickness+alpha
    still track progress.
  - **SCROLL/GESTURE PASSTHROUGH TO NEXT PAGE V32.34+ (the real feature).**
    Horizontal carousels (e.g. Settings theme carousel) used to SWALLOW the
    swipe at their edge — once classified ChildScrollH the navigator deferred
    entirely, so "scroll to the end, keep swiping → next page" did nothing.
    Now wired end-to-end:
      * WHEEL/trackpad — HorizontalScrollArea::wheelEvent applies the delta but
        if the scrollbar was ALREADY clamped at the edge it's pushing toward
        (value didn't change), it ignore()s + QScrollArea::wheelEvent so the
        event propagates to SwipeNavigator's wheel handler for the page handoff.
        Self-contained, safe, flag-agnostic. (This is the path that matters on
        the M2 Ultra dev machine.)
      * TOUCH, no h-range — a "carousel" whose content fits (maximum==minimum)
        no longer defers; falls through to resolve as a normal PageSwipe.
      * TOUCH, scrollable (unifiedNavEnabled) — the navigator now OWNS the
        gesture instead of deferring: enters Ambiguous remembering m_scrollTarget.
        resolveAmbiguous: on dominant horizontal motion, if the carousel can
        still scroll the dragged direction → resolve ChildScrollH (scroll it);
        if it's clamped at the edge in that direction → fall through to the
        PageSwipe block (the handoff). ChildScrollH now SHARES the ChildScrollV
        TouchUpdate + TouchEnd/fling dispatch (which already H-axis-locks,
        routes edge overflow to applyOverscroll, and flings horizontally) — so
        the carousel scrolls + flings via the proven path, and m_axisLock=
        Horizontal makes the release fling H-only.
      * Flag OFF preserves the EXACT old behavior: scrollable ChildScrollH still
        defers entirely to the child at classification (the !unifiedNavEnabled
        early return), so no risk to existing carousel scroll when the flag's off.
    DESIGN NOTE: the handoff fires on a FRESH drag started at the edge (no
    mid-gesture reclassification — the design prohibits that; scrolling to the
    edge mid-drag rubber-bands instead). That's the safe, predictable rule.
    COMPILE-CORRECTNESS: removing ChildScrollH from the two no-op switch groups
    was required — it was previously a no-op case, and adding it as a
    ChildScrollV fall-through would otherwise duplicate the label (a real
    compile error braces can't catch; caught via a case-label grep sweep).
    NEEDS DEVICE: the touch handoff threshold + feel are unverified.
    SCOPE (verified by inspection): HorizontalScrollArea is instantiated in
    exactly ONE place inside the page stack — SettingsPage:840, the theme
    carousel. SnapComposerDialog/DiagnosticsDialog use it but are MODAL (outside
    the SwipeNavigator), so the touch passthrough's only real-world effect is on
    the Settings → theme carousel. Test the handoff THERE: scroll the theme row
    to its end, keep swiping → should land on the adjacent page (Wayfinder going
    right / Friends going left). The owned-ChildScrollH state is fully cleared by
    resetGestureState (m_intent/m_axisLock/m_passingToScrollArea/m_scrollTarget),
    which the shared ChildScrollV TouchEnd case + the 2.5s watchdog both call —
    so a stalled/owned H gesture can't get stuck. No m_intent==ChildScrollV
    comparisons exist outside the (now H-inclusive) switch cases, so nothing
    misses the owned-H intent.
    ▶ V32.34+ DIAGNOSTICS ADDED (device run 23:17 showed ZERO gesture activity
    in the log → likely the flag was off). The gesture traces were all qCDebug
    (invisible in release). Now at INFO level (always in the log):
      • Startup: "[UNIFIED-NAV] ENABLED/disabled (source: ...)" — printed once
        the first time unifiedNavEnabled() is queried. CHECK THIS FIRST: if it
        says "disabled", the sub-tab detent + passthrough are inert by design,
        which fully explains "handoff not implemented yet". Launch with
        SC_UNIFIED_NAV=1 (env) — e.g. `SC_UNIFIED_NAV=1 ./snapclone` — or set
        QSettings ui/unified_nav=true.
      • "[HANDOFF] scroll→sub-tab: advanced NEXT/PREV (rawY=… >= 220)" when the
        vertical detent fires (scroll-to-end → sub-tab).
      • "[HANDOFF] sub-tab scrub WRAPPED … → breaks to page" on the horizontal
        in-bar scrub wrap.
      • "[HANDOFF] carousel at edge → passing through to page swipe" when a
        scrollable carousel hands off to a page change.
      • "[HANDOFF] swipe→page: committed to page N" when a swipe commits a page.
      • "[GESTURE] touch classified as X" — ONE line per touch-down (gated on
        the flag, so silent in the default build). X ∈ {PageSwipe, ChildScrollV,
        ChildScrollH, PassToChild, TapCommit, Ambiguous}. This is the MIDDLE of
        the story: a swipe that does nothing will show what it classified as —
        PageSwipe (should've navigated → bug downstream), ChildScrollH/V (the
        touch landed on a scroll area), or PassToChild (hit a button/interactive
        widget that ate it). The classification + the [HANDOFF] presence/absence
        together localize any non-working gesture precisely.
    If a gesture produces NO [HANDOFF] line, the classification isn't reaching
    the handoff — that's the thing to debug, and the absence localizes it.
  - **BAR HIGHLIGHT RE-SYNC ON SHOW V32.34+ (defensive, latent-gap insurance).**
    Audited whether the bar's active-segment highlight can go stale: the
    navigator's subTabChanged covers gesture/keyboard changes, the bar's own
    tap handler self-refreshes, and STARTUP restore is fine (pages restore their
    sub-tab in their ctor at MainWindow:213-214, BEFORE the bar is built at 258,
    so the bar's first rebuild reads the restored state). The only uncovered
    case is a FUTURE programmatic filter switch from page code (none today —
    confirmed no runtime callers outside bar/navigator), which wouldn't fire
    subTabChanged. Rather than wire 3 speculative page signals for a gap with no
    current trigger, added a cheap zero-coupling catch-all: UnifiedNavBar::
    showEvent re-runs refreshActiveSegment(), so the highlight re-syncs from the
    live SubTabbed model whenever the bar (re)appears. Covers the hypothetical
    without coupling the bar to each page's internals.
  - **L3 SHORT-FORM SMARTS V32.34+.** The narrowest collapse level used a crude
    "first 3 chars" (About→ABO, Online→ONL). Now it prefers a SINGLE uppercase
    initial (clean, like a tight segmented control) when the page's labels have
    DISTINCT first letters — checked via a QSet pass — and only falls back to
    3-char when initials would collide (e.g. Sent/Storage). All three current
    SubTabbed pages have unique initials (Inbox A/U/S, Contacts A/O/P, Settings
    G/C/N/S/A), so they all get the clean single-letter form; the full label is
    always in the tooltip. NOTE on the subTabIcon hook: decided NOT to wire it
    for Settings — only Camera→camera.svg maps cleanly; General/Network/Storage/
    About have no asset, so a one-icon-four-letters row would look arbitrary.
    The improved short-form is the better no-new-assets answer; the icon hook
    stays available for when/if real sub-tab SVGs get authored.
  - **BAR POLISH V32.34+ (tooltips for collapsed segments + arrow hints).**
    Two small usability fills: (1) when the responsive collapse reduces an
    inactive segment to an icon / 3-char abbreviation on a narrow screen, the
    segment now carries a TOOLTIP with its full label so the meaning isn't lost
    (cleared at L0–L2 and for the active segment where the text is visible);
    (2) the page prev/next arrows now show adjacent-page-name tooltips
    ("Previous: Inbox" / "Next: Friends") and clear them at the ends. Also
    audited Contacts + Settings for the same orphaned-action regression the
    Inbox had — neither used the SmartRibbon, so no lost actions; Settings'
    own bottom actionBar (Test/Reset/Factory/Save) is independent page content
    and unaffected by hiding its tab strip (it already has its own BtnCollapser
    icon-collapse, mirroring the bar's responsive rule — good precedent).
  - **PER-TAB ACTIONS → PAGE CONTENT V32.34+ (design choice: actions out of
    the nav bar).** The old SmartRibbon prototype put "Mark all read" (Unread)
    and "Refresh" (Sent) on the bar; under unified nav the SmartRibbon isn't
    built, so those actions had no home. Per the chosen design (keep the bar
    nav-only; actions live in page content; revisit an overflow ⋮ later if it
    feels cramped), InboxWidget now has a slim right-aligned CONTEXTUAL ACTION
    ROW just below the filter tabs: it shows "Mark all read" on the Unread
    filter (only when m_unreadCount>0), "Refresh" on Sent (only when there are
    outbound rows), nothing on All. refreshContextualAction() drives its
    label/visibility and is called from applyFilterIndex (filter switch),
    markAllConversationsRead (hides after clearing), and the unread-tally
    update (live show/hide). Dispatches to the same handlers the SmartRibbon
    used. Shown regardless of nav flag (it's page content). The SmartRibbon's
    own action wiring is left intact behind its separate SC_SMART_RIBBON flag
    (redundant but harmless; not deleted — avoids the styled-widget teardown
    hazard). FOLLOW-UP if testing shows it's cluttered: collapse to an overflow
    ⋮ menu (the option-3 fallback).
  - **SUB-TAB PRESERVATION ON PROGRAMMATIC NAV V32.34+ (correctness fix).**
    The "land on a new page's first sub-tab" reset was firing on EVERY page
    change incl. programmatic goToPage() calls — the startup default
    (goToPage(kPageInbox, animate=false)) and the "jump to inbox after sending"
    / "jump to friends to add" calls — so a programmatic jump slammed the
    page's sub-tab to 0, fighting e.g. the Inbox's persisted-filter restore at
    launch. Fixed by adding `goToPage(index, animate, resetSubTab=false)`: the
    reset now happens ONLY for user navigation (bar prev/next arrows, keyboard
    arrows/Ctrl+Tab, and the swipe break-through which always resets via its
    own startCommitAnimation path). Programmatic jumps default to PRESERVING
    the destination page's current/restored sub-tab. Existing one-arg callers
    (ComponentTestWindow, the dormant-under-unified TabBar tap) are unaffected
    — both new params default. The horizontal-swipe break-through is unchanged
    (separate code path, always resets — correct).
  - **PAGE-POSITION DOTS + EMPTY-PAGE POLISH V32.34+.** Two cohesion fixes for
    the bar across all 5 pages:
      * PAGE DOTS — paintEvent now always draws a centered row of page-position
        dots along the bar's TOP edge (current page = filled accent pill, others
        dim), every page. This is the global "where am I among the N pages"
        indicator the directive's `| NEXT PAGE ||` gestured at; it also stops
        Camera/Wayfinder (no sub-tabs) from looking hollow. Painted (no
        widgets/stylesheet) like the underline + badges.
      * EMPTY-PAGE NAME CENTERED — on non-SubTabbed pages (Camera/Wayfinder) the
        fixed left page-name chip is hidden and a CENTERED name is dropped into
        the stretchy segment row (stretch | NAME | stretch), so the bar reads as
        a deliberate "‹  CAMERA  ›" rather than a left-aligned name with a
        hollow gap. The underline + count-badge paint paths are guarded to real
        SubTabbed pages (st != null) so nothing draws under the centered name.
      * STRETCH-LEAK FIX (latent bug found while doing the above) —
        clearSegments() only removed the tracked buttons, leaving the
        addStretch() spacer items in the segment row to ACCUMULATE one (or two,
        on empty pages) per rebuild, progressively squeezing the segments over
        repeated page changes. Now drains every layout item via takeAt(0) —
        spacers deleted, button widgets deleteLater'd. Pre-existing since the
        bar was built; the empty-page path (2 stretches) would have made it
        visible faster.
  - **SUB-TAB BADGES RESTORED V32.34+ (regression fix from the unification).**
    Hiding the top SegmentedTabStrips removed the count pills they showed —
    Inbox "Unread" (unread-conversation count), "Sent" (in-flight outbound),
    Contacts "Online" — so those numbers had vanished from the UI even though
    the pages still implement subTabBadge(). The unified bar now PAINTS those
    pills (accent circle + TEXT_INVERSE count, top-right of each segment),
    reading counts live from SubTabbed::subTabBadge() on paint. Painted, not
    overlaid widgets / setStyleSheet, to keep the drag repaints cheap and off
    the QStyleSheetStyle caches. Live updates: UnifiedNavBar::refreshBadges()
    (just an update() trigger) is wired in MainWindow to InboxWidget::
    unreadCountChanged and a new ContactsPage::badgesChanged signal, so a
    freshly-arrived unread / a peer going online bumps the pill while the user
    sits on that page; tab/page switches already repaint. >99 shows "99+".
  - **IN-FLIGHT DRAG-WEIGHT BUILT V32.34+ (the safe way).** The bar now
    previews the pending sub-tab move DURING a horizontal drag, not just on
    commit. SwipeNavigator emits `subTabDragProgress(int dir, qreal progress)`
    from the gestureUpdate friction block (dir +1 toward next / -1 toward prev;
    progress 0..1 toward the sub-tab commit point), plus a progress-0 emit at
    gestureEnd to drop the preview. UnifiedNavBar::onDragProgress stores it and
    repaints; paintEvent draws a RADIUS_SM-rounded accent underline that at
    rest sits under the active segment and during a drag slides/stretches
    toward the neighbour by `progress`, fading up slightly as it goes. CRUCIAL:
    this is a paintEvent indicator — the ~60Hz drag stream NEVER calls
    setStyleSheet, so it stays off the QStyleSheetStyle caches that the
    shutdown teardown crash is sensitive to. (The earlier-deferred risk is thus
    avoided by construction.) FEEL: the underline slide rate + the
    0.55→0.90 alpha ramp are tweakable seeds — tune on-device alongside the
    detent constants.
  - **SCROLL-PASSOVER FIX V32.34+ (this was the bug making vertical passover
    feel dead).** `applyOverscroll` was ONLY called from `onFlingTick` — so a
    DIRECT DRAG to the content limit and continued pull produced NO overscroll
    and never armed the vertical detent; only a kinetic FLING into the edge
    did. Added `commitScrollDelta(hbar,vbar,dx,dy)` that applies the scroll and
    captures the clamped OVERFLOW → applyOverscroll. Both commitScrollPaced
    commit branches now route through it; flushPendingScroll (TouchEnd) also
    routes its final delta + springs back any drag-accumulated overscroll
    (the fling path springs back on its own; a pure drag had no tick to). Now
    "scroll to the bottom and keep pulling" arms+fires the sub-tab detent as
    designed.
  - VERTICAL: extend the overscroll handler — barrier SVG opacity tracks
    `min(1, overscrollRawY / detentThreshold)`; cross threshold → advance
    sub-tab + reset overscroll (no spring-back, or a confirming snap).
  - HORIZONTAL: sub-tab scrub via slide offset; at cycle boundary, the
    rubber-band resistance ramps + barrier fades; past threshold →
    page-commit landing on sub-tab 0.
  - Desktop: shared `advanceSubOrPage`; trackpad two-finger maps to delta
    when the bar subregion has focus.
  - Keep behind `SC_UNIFIED_NAV` until device-validated — touches core
    navigation. Friction coefficient + detent threshold + barrier-fade
    timing are FEEL parameters → build-test-adjust loop on-device, not
    reasoned. Start the constants near the existing overscroll values
    (kOverscrollRubberK=0.4, kSpringBackMs=220) and tune from there.


- [x] ~~**🌟 Conversation-grouped inbox — promote V32.15 backend work to the
  UI.**~~ **Done V32.20.** `ConversationDetailDialog` shipped, `InboxWidget`
  extended additively with `setSelfId` / `setStorage` /
  `reloadConversationsFromStorage`. Detail dialog uses
  `loadConversationSnaps()` for the thread, list items now carry the
  other-party UserId in UserRole and drill down on tap. `openSnapById`
  extracted as a private helper so dedup-map + viewer-launch logic
  remains in one place across flat-list and detail-dialog paths.
  Pending real-device test for: tab-bar unread badge accuracy after
  reload, conversation-row sort order after a long offline batch,
  detail-dialog scroll-to-bottom on open. See WORK_LOG V32.20 for the
  full design + risk notes.
- [ ] OutboxWidget exists but is unmounted. Decision needed: 6th
  swipe tab? Peek panel from inbox? In-inbox section?
  *(Note: largely subsumed by conversation-grouped inbox above — 1:1
  outbound snaps will appear inside threads. Reconsider after V1.)*
- [x] ~~**Edit-contact button: icon + mistap-safe trigger.**~~
  **Done V32.24.** `assets/icons/edit.svg` Feather-style pencil
  shipped, registered in qrc, button uses `SvgIcon::secondary` for
  theme-tinted rendering. Long-press (500ms) opens the editor; short
  tap surfaces a transient QToolTip "Tap and hold to edit". The
  hold detector is a per-button QTimer parented to the button. Real-
  device verification on FP5 still pending.
- [x] ~~**Contacts not loading immediately at startup.**~~ **Done
  V32.24.** Mirrored the V32.20 inbox-init pattern: right after
  `m_inboxPage->reloadConversationsFromStorage()` in MainWindow.cpp,
  `m_friendsPage->setContacts(storage->loadContacts(...))` runs at
  init. `loadContacts` is a single indexed SELECT — milliseconds,
  not the slowness source. The missing init call was. If real-
  device testing still shows perceptible delay after this lands,
  consider the shared `Contact::List` model abstraction; otherwise
  the obvious fix is sufficient.
- [ ] Link composer URL preview — Rust `linkpreview::fetch` exists,
  no Qt UI hookup
- [ ] Wayfinder NV12 viewfinder explicit YUV path now in place — needs
  on-device test (Asahi/aarch64 desktop reported tiled garbage previously)
- [ ] Wayfinder: account for multi-LAN peer locations (same SSID
  could be home or office)
- [x] ~~🧪 **Wayfinder shows nothing after receiving snaps from peers.**~~
  **Done V32.28.** Investigation: SnapRouter's
  `cacheSnapSenderLocation` was already writing inbound snap locations
  to `peer_locations` correctly. The gap was on the UI side —
  `WayfinderPage::onSnapReceived` only updated existing in-memory
  nodes; if the peer wasn't yet on the map (first time sharing a
  location), the pin didn't appear until the 30s refresh tick or
  page re-enter. Fix: when a snap arrives from a peer NOT currently
  in `m_nodes` AND it carries a location, fire `refreshLayout()`
  immediately. Existing-node path keeps its pulse-only repaint
  (no layout rebuild) to avoid avatar reloads on every snap.
- [ ] 🧪 **Page-switch smoothness: investigate debouncing / other UI
  tactics for stutters during swipe transitions and passthrough.**
  *V32.26 ships suspect #1 (show/hide event storms → camera teardown).
  CameraWidget showEvent/hideEvent now route through a 250ms debounce
  timer. A fast swipe past the camera page (show + hide within 250ms)
  cancels the queued start, so the gstreamer backend isn't torn down
  and rebuilt mid-swipe. Only real visibility transitions reach
  startCamera/stopCamera. The other four suspects (Wayfinder paint
  during animation, synchronous signal cascades, tab-bar badge
  flashes, swipe-passthrough touch leakage) are still open — pending
  real-device verification that the camera debounce alone moves the
  needle, since they may be downstream of the same root cause.*
  Observed but not yet measured: page changes (especially camera ↔
  inbox and inbox ↔ wayfinder) can stutter on mobile and during fast
  successive swipes. Possible culprits to investigate before deciding
  on a tactic:
    - **Show/hide event storms.** `SwipeNavigator::pageChanged` cascades
      `showEvent`/`hideEvent` to every page; CameraWidget starts/stops
      its gst pipeline on these. A fast double-swipe past the camera
      page can trigger a tear-down-then-rebuild that takes ~1.5s
      (V32.6 kernel-release wait). Debouncing the visibility-driven
      lifecycle (apply only after the page has been "settled" for
      e.g. 80ms) might eliminate the worst of it.
    - **Heavy paint() during the animation frame.** WayfinderPage's
      map paints potentially-many markers each frame; if its update()
      runs during the transition's QPropertyAnimation, frame budget
      blows. Worth profiling whether `Qt::WA_OpaquePaintEvent` is set
      and whether the paint region is full-widget vs an explicit
      QRect. (Already filed separately under "Wayfinder map update()
      vs repaint region" in Polish.)
    - **Synchronous signal cascades during pageChanged.** Currently
      `pageChanged` triggers: UxGuide milestone drain, edge-hint
      state update, AppContext::setCurrentPage (V32.21), TabBar
      sync. Each of these can run lambdas that allocate/repaint.
      Posting the non-urgent ones via `QMetaObject::invokeMethod`
      with QueuedConnection would let the animation frame complete
      before they run.
    - **Tab-bar badge flash.** The unread-count update fires
      `setBadge` which triggers a TabBar repaint. If a snap arrives
      mid-swipe, the badge repaint happens inside the animation —
      visible as a tab-bar jitter. Could queue badge updates while
      `m_swiper->isAnimating()` is true.
    - **Passthrough specifically:** swipe-passthrough (V32.7?) is the
      gesture where a touch starts on one page and ends on the next.
      The grab/release transition is when stutters are most visible.
      Check whether the touch event is delivered to BOTH pages briefly,
      and whether either page does work in mousePressEvent that
      should defer to mouseReleaseEvent or onClicked.
  **Tactic candidates** (decide after profiling):
    - Debounce showEvent/hideEvent → camera lifecycle (most-likely win)
    - Queue tab-bar badge updates during animation
    - Move AppContext::setCurrentPage to a queued post (low cost,
      defensive)
    - Disable WayfinderPage `update()` during active swipe animation
    - Look at SwipeNavigator's own `QEasingCurve` — if it's
      InOutQuad the slowdown at endpoints amplifies any single-frame
      glitch; OutCubic gives less time to notice
  Goal: identify which one (or two) actually moves the needle on
  device, then ship. Don't add all four — debouncing show/hide and
  also queuing every callback adds latency without reason if only
  one was the real culprit. Pair with on-device frame-time logging
  (Qt has `QQuickWindow::frameSwapped` analogue for QWidget; the
  cheap version is logging the QElapsedTimer between pageChanged
  emit and the next paintEvent on the target page).

#### V32.34+ session items

- [x] ~~**Wayfinder shows nothing for contacts on same network.**~~
  **Fixed V32.34+.** Two compounding bugs in `loadContactsAndLocations`:
  bailed entirely when `m_haveSelfFix` was false, AND skipped contacts
  without a stored `peer_location` row. LAN-only peers without GPS data
  never appeared. Now contacts without a location are rendered in a
  "nearby" cluster around the self-center, with distanceLabel reading
  `Nearby` (online) or `Offline`. Self-fix is no longer a hard
  precondition for the page to render anything.

- [x] ~~**Edit-contact dialogs stack on top of each other.**~~
  **Fixed V32.34+.** `ContactsPage::openEditor` now tracks an
  `m_openEditor` QPointer; if a dialog is already alive, subsequent
  taps raise it instead of spawning a new one. The previous code's
  comment claiming "redundant B is a non-issue because A is on the
  modal stack" was empirically wrong on Phosh — modal stacking is
  inconsistent and the user saw a confusing pile of identical sheets.

- [x] ~~**5% global scale reduction.**~~ **Applied V32.34+.** All
  `Theme` constants (TOUCH_*, FONT_*, SPACE_*) dropped by ~5%, rounded
  to nearest integer. TOUCH_MIN remains above Material's 32px floor.
  SPACE_XS held at 6 (already minimal). Hardcoded local sizes were NOT
  touched — if any specific component reads too tight after this, fix
  it at the local site.

- [x] ~~**Edit-contact dialog wider than phone screen.**~~ **Fixed
  V32.34+.** ContactEditorDialog used a hardcoded `resize(380, 560)`
  which clipped the right edge on 360px-wide phones (FP5), making the
  Save button at the bottom-right unreachable. Now reads the primary
  screen size at construction and resizes to 92% width × 80% height,
  capped at 420×640 to avoid growing absurd on desktop.

- [x] ~~**Mobile display debug toggle.**~~ **Done V32.34+.** New
  `UiDebugFlags` helper persists settings under `debug/mobileDisplay`
  and applies `QLoggingCategory::setFilterRules` to gate the chatty
  categories (`snapclone.ui.vkeyboard`, `snapclone.ui.modal`,
  `snapclone.ui.modaltest`, `snapclone.dialog`). Checkbox added to
  Settings → Debug & FSM Tuning. Off by default — these logs are
  noise outside of active diagnosis. Applied at app start via
  `UiDebugFlags::apply()` from main.cpp and live-applied when toggle
  flips (no restart needed).

- [x] ~~**ModalLayerTestPanel L3 dialog: tap-anywhere-to-close.**~~
  **Implemented V32.34+.** The test widgets now close on a tap inside
  them in addition to the modal-surface tap-outside-dismiss machinery.
  Critical-layer (L6) is exempt because its whole testing purpose is
  to verify the explicit-Close-button-only behavior. This addresses
  the "L3 dialog doesn't close on tap" report — the user was tapping
  inside the colored card (the obvious affordance), and the modal
  surface only listened for outside-taps.

- [ ] **Diagnose any remaining tap-outside-dismiss failures in
  ModalSurface.** With UiDebugFlags' mobile display toggle ON, the
  modal surface logs every tap with `inside=` flag and layer name.
  If a future failure report is filed, the user should enable the
  toggle, reproduce, and attach the log. The eventFilter handles
  both MouseButtonPress (mobile synthesis) and TouchBegin, so the
  most likely remaining failure mode is a z-order issue where a
  sibling widget intercepts the tap before reaching ModalSurface.

- [x] ~~**Camera green-tint: more robust caps negotiation.**~~
  **Strengthened V32.34+.** Added an explicit 4-byte-format caps
  filter (`format={BGRx,BGRA,RGBA,RGBx}`) right after the first
  videoconvert. Previously the negotiation could land on a 3-byte
  format mid-pipeline that downstream elements passed through
  without conversion, producing the off-stride green mess. Now
  upstream is forced to a 4-byte format and the final videoconvert
  + caps filter handle the BGRx coercion deterministically.
  Different pmOS edge builds had different default negotiations,
  which explains the "one phone tinted, the other fine" reports.

- [ ] **Monthly export reminder UI** (data model done; UI deferred).
  V32.34+ shipped:
    - `MonthlyExportReminder` helper with QSettings-backed state
      (lastShown YYYY-MM, autoSyncPath, snooze flag).
    - Settings → Save & Export Snaps section with auto-sync path
      input. Auto-sync replaces the monthly prompt entirely.

  **Still TODO:**
    - The actual reminder card on Inbox or a transient overlay.
      Should show recent-snap thumbnails (a few from the user's
      inbox + outbox) and offer two buttons: "Export as zip" and
      "Maybe later".
    - The zip export action itself — collect all snap media files
      from the storage manager's media folder, zip into a single
      file the user can save / share. Reuse the existing share
      sheet (mobile) or QFileDialog (desktop).
    - Hook on app start: if `MonthlyExportReminder::shouldShowThisMonth()`
      returns true AND the inbox has at least N snaps from the last
      30 days, show the card. Below the threshold, skip — no point
      reminding a user who has nothing yet.

- [ ] **Stack-style navigation for sub-sections** (designed earlier,
  still not built — referenced in the previous V32.34+ session block).

- [ ] **Stack-style navigation for sub-sections.** User asked for a
  pattern where opening a sub-area (Settings sub-section, contact
  detail, attachment view) REPLACES the bottom TabBar's current tabs
  with the sub-area's tabs, AND replaces the swipeable pages with
  sub-section peers. Lets users swipe between sub-sections without
  losing the bottom-bar gesture pattern.

  **Design sketch (deferred to its own session):**

  - Add `TabBar::pushTabSet(QList<TabSpec>)` / `popTabSet()` to
    support an alternate tab configuration stack. Default state is
    the top-level pages; pushing a new set animates the tab icons
    swapping in.
  - `SwipeNavigator::pushSubPages(QList<QWidget*>)` /
    `popSubPages()`. Subbed-in pages occupy the same swipe area
    visually but logically own a child SwipeNavigator. The parent
    stays paused while the sub-stack is active.
  - Back-gesture (edge swipe from left): pops the top sub-stack
    frame.
  - First user: Settings sub-sections (Account / Camera / Network /
    Storage / About). Each becomes a peer page; the existing
    scrolly Settings becomes a router that prepares the sub-stack
    based on which row was tapped.
  - Second user candidate: Contacts → contact-detail. Tapping a
    contact pushes the detail as a peer page. Attachments view
    becomes a further sub-push from contact-detail.

  **Risks:** the SwipeNavigator's gesture machinery was designed
  for a flat list of pages. Nested navigators introduce ambiguity
  about which gesture handler "owns" a touch on a sub-page. Likely
  needs a Z-axis lock similar to the V32.33+ axis-lock work.

### Video
- [x] ~~**Exported videos seek/scrub coarsely — no forced I-frames in
  the transcode.**~~ **Fixed V32.34+.** All four export encoder backends
  (ffmpeg-libsvtav1, gst-svtav1enc, gst-vp9enc, gst-vp8enc in
  `VideoEncoderBackend.cpp`) re-encoded the recorder's intermediate file
  with NO keyframe-interval control, so each AV1/VP9/VP8 encoder picked
  its own long default GOP. A keyframe cadence does NOT survive a
  re-encode — the recorder's `key-int-max=30` / `video_gop_size=30`
  applies only to the intermediate file, and the transcode picks a fresh
  GOP from scratch — so exports had I-frames only every several seconds.
  Players can only seek to keyframes, so scrubbing jumped in big chunks
  and the first frame decoded slowly. Fix: force ~1s keyframe interval on
  the user-facing tiers (Standard/HighQuality; thumbnails/previews skip
  it since they're tiny and rarely scrubbed, and extra keyframes cost
  bitrate). ffmpeg path uses `-force_key_frames expr:gte(t,n_forced)`
  (time-based, fps-independent, authoritative — doesn't rely on
  EncodeSpec carrying fps, which it doesn't); the gst svtav1enc path uses
  `parameters-string=keyint=30` and the vp9/vp8 paths use
  `keyframe-max-dist=30` (frame-based; 30 ≈ 1s at the recorder's known
  30fps). Device-verify: export a clip, scrub it in the viewer — seeking
  should land close to the target instead of snapping seconds away, and
  the first frame should appear quickly.

- [x] ~~**Face/saliency-aware ROI bitrate boosting in exports.**~~
  **Done V32.34+ (static-box variant).** Followed up on the analysis
  below by shipping option (a): the ffmpeg export backend now boosts the
  salient (face/subject) region. `VideoTranscoder::detectSalientRoi()`
  extracts one representative frame (~1s in, via `ffmpeg -ss 1 -frames:v
  1` to a ≤512px temp PNG), runs the existing `sc_saliency_bbox`, maps
  the box from extract space to source pixels (second short ffmpeg probe
  for source dims), and stores it on `EncodeSpec.roi*`. The
  ffmpeg-libsvtav1 backend prepends `addroi=x:y:w:h:-1` (qoffset −1 = max
  quality boost) to the -vf chain BEFORE scale, so the encoder spends
  more bits on the face and compresses the background harder. Gated to
  the ffmpeg backend (only one with an addroi equivalent) and the
  Standard/HighQuality tiers. Null ROI (saliency off, extract failed, no
  subject) → roiW stays 0 → backend skips the filter. This is the cheap
  static-box win; the moving-subject (per-frame ROI) case still wants the
  in-process libav* rewrite — see note below, kept for that future work.
  **Original analysis (retained for the per-frame future work):**
  The export path is CLI-based (ffmpeg / gst-launch subprocesses), and
  per-frame ROI bitrate allocation is NOT well-exposed on either CLI.
  ffmpeg ROI needs per-frame AVFrameSideData via the libavfilter `addroi`
  filter (static box, or a scripted sendcmd timeline); SVT-AV1's ROI
  segment maps aren't plumbed through the ffmpeg CLI at all. Doing it
  properly means either (a) a static saliency box applied to the whole
  clip via `addroi` (cheap, but only helps if the subject doesn't move),
  or (b) moving the encode in-process onto libav* APIs with per-frame
  saliency (large; abandons the crash-isolated subprocess model).
  Recommend (a) as a future cheap win: run sc_saliency_bbox on the
  thumbnail frame we already extract, pass it as a single
  `addroi=x:y:w:h:-1` (qoffset −1 = max boost) filter in the ffmpeg
  backend for Standard/HighQuality. Filed rather than built because a
  static box has narrow benefit and the moving-subject case needs the
  in-process rewrite.

- [ ] 🧪 **Recorded .webm files are corrupted on the vp8/webm fallback
  path.** *V32.25 ships a defensive output-grace guard in `stop()`:
  before terminating gst-launch, if the output file is still 0 bytes
  and total recording time is under 1.5s, we sleep-poll the file
  size for up to 1500ms total before sending SIGTERM. This catches
  the common case (user double-taps shutter, pipeline never reached
  PLAYING). The post-mortem `size < 1024` check at the end of the
  `finished()` handler still flags pipelines that produce nothing
  after the grace window — better a clean "recording failed" than
  a corrupt webm shipped to the inbox. Pending real-device verify
  that this actually fixes the symptom on the FP5 vp8 path; if the
  vp8enc/webmmux pipeline really takes longer than 1500ms to write
  its first header byte on slow ARM, we'll need to bump the grace.*
  ffprobe on a fresh recording from FP5/Alpine reports
  `EBML header parsing failed` — i.e. the webm header was never
  written. Diagnosis from the ffprobe output:
    - The old `.mkv` recording (V32.6-era, H.264 in matroska) was
      well-formed: ffprobe parsed it, reported h264 + opus
      streams, even the rotation matrix.
    - The new `.webm` recording (vp8 + webmmux) has no parseable
      EBML at all — the bytes at the start of the file aren't a
      valid webm header.
  **Why playback "works but stutters":** the viewer uses `decodebin`
  which is more tolerant than ffprobe. It can pull *some* frames
  out of a malformed container (audio packets are smaller and
  more frequent, so audio plays close to normal; video keyframes
  are sparse so video stutters or skips). User's intuition that
  "videos are AV1" is half-right: the *transcoded* output is AV1,
  but the *raw recording* (which gets transcoded later) is VP8 on
  this device because x264enc / openh264enc weren't available.
  **Most plausible root cause:** the recorder process is killed
  before the pipeline reaches PLAYING state. Current code at
  `VideoRecorder.cpp` line ~501 does `m_proc->waitForStarted(2000)`
  — that confirms gst-launch *started* (process is alive) but NOT
  that the pipeline reached PLAYING. There's a gap of 100-500ms
  between process spawn and pipeline-streaming on slow ARM. If the
  user records very briefly (< ~500ms) and hits stop, the pipeline
  never even produces a header before SIGTERM. Webmmux writes its
  header on the first cluster — no cluster, no header, corrupt file.
  **Cheapest fix:** parse gst-launch's stdout for the "Setting
  pipeline to PLAYING" message (gst-launch with `-m` flag prints
  state changes), and only mark recording as "really recording"
  after that. Block `stop()` from terminating until either
  PLAYING was seen OR a longer timeout (5s) fires.
  **Alternative:** enforce a minimum-frames-written before allowing
  stop. Check `multifilesink`'s preview-JPEG existence as a proxy.
  Pair this with the audio-normalization work below — both are
  pipeline robustness improvements that should land together.
- [ ] **Audio normalisation + compressor on encode.** *V32.32 ships a
  simplified version* — `+3dB volume` followed by `audiodynamic`
  configured as a hard-knee limiter at threshold 0.95, ratio 20.0
  (effectively a -0.4dBFS ceiling) before `opusenc`. Catches quiet
  recordings, prevents clipping on loud ones. Single-pass, no
  analysis stage. The fuller spec below is still open:

  Recordings vary
  wildly in level (loud nearby talker vs quiet ambient capture). Goal:
  ship every video at a consistent loudness so playback doesn't blow
  out the listener after a quiet snap, and so quiet snaps aren't lost
  in the noise floor.

  **Targets:**
    - Compressor: ratio 1.5:1, threshold -25 dB. Gentle — we're not
      mastering for radio, just preventing the worst peaks from
      dominating.
    - Auto-gain to hit integrated loudness target of -14 LUFS (the
      streaming/YouTube de-facto standard). Don't aim for -23 LUFS
      (broadcast spec) — too quiet for casual phone playback.

  **Implementation sketch:**
    - GStreamer has the pieces: `audiodynamic` (compressor),
      `volume`, and the EBU R128 loudness analyser via
      `level` / `rganalysis` / `ebur128` (whichever the platform ships).
      Cleanest single-pass option is `audiodynamic` for the compressor
      + `rgvolume` for gain ride.
    - Two-pass would be more accurate (analyse loudness first, then
      apply makeup gain) but doubles encode time; defer.
    - For tier-aware encoding (V32.4 / V32.17 work): apply at the
      Standard tier only initially. Thumbnail tier doesn't need it
      (no audio); High tier could skip and let the receiver normalise
      on playback if we go that direction.

  **Pipeline shape** (the audio branch in
  `GstSvtAv1Backend::buildArgs()` and the other encoder backends):
    ```
    dec. ! queue ! audioconvert ! audioresample
       ! audiodynamic mode=compressor ratio=1.5 threshold=0.056234
       ! rgvolume target-level=-14
       ! audioconvert ! opusenc bitrate=64000 ! ...
    ```
    `threshold=0.056234` is the linear amplitude for -25 dBFS
    (10^(-25/20)). Verify the actual element parameter names against
    the local GStreamer version before shipping — these have changed
    across major releases.

  **Why now-ish:** ad-hoc user feedback already shows the variance is
  audible. The cost is small (one extra element pair in the encode
  pipeline) and the user-facing win is large (no more startle-jumps
  when a loud snap follows a quiet one). Pair with a quick A/B test
  on a few sample recordings before shipping to confirm we're not
  making things worse on already-well-recorded inputs.
- [ ] Hardware-accelerated encode/decode where available (V4L2 M2M
  on Fairphone's exynos-derived video driver)
- [ ] Adaptive bitrate based on measured throughput to peer
- [ ] Transcoder uses `gst-discoverer-1.0` which can be slow on
  mobile (~5s); consider caching the probe per-recorded-file via
  a small JSON manifest
- [x] ~~Live viewfinder preview during recording~~ — done in V32.6
  via tee + multifilesink JPEG. Follow-ups below:
- [ ] **Preview latency during the 1500 ms kernel-release wait at
  recording START.** No preview JPEG exists yet during this window —
  user still sees the static "RECORDING" placeholder until the first
  frame hits the filesystem. Could poll `multifilesink`'s first
  write event and only THEN show the placeholder if no frame appears
  within ~250 ms. Or: skip the placeholder entirely and rely on the
  preview JPEG to replace the stale Qt frame.
- [ ] **Considered alternatives for live preview, kept as fallbacks**
  in case the tee approach has issues on some platform: (2)
  xvimagesink / glimagesink rendering into an embedded QWindow —
  lower latency but X11/GL-specific; (3) PipeWire bidirectional
  stream where the recorder publishes a node the viewfinder can
  subscribe to — cleanest but excludes the v4l2-direct path on
  Asahi desktops.
- [x] ~~**Progress meter goes 0% → "Done"** after V32.3 speedup. The
  time-based fallback creeps 1%/3s but the encode now finishes in
  <3s for short clips on desktop, so the user only ever sees 0.~~
  **Done V32.32.** Combined (a)+(b) from the original options:
  first-stderr trigger jumps to 5% the moment the encoder emits
  any output (banner, version dump, anything), and the time-based
  estimate switched from linear `1%/3s capped 60` to exponential
  `60*(1 - exp(-t/4s))` — front-loaded so it hits 7% at 500ms,
  13% at 1s, 24% at 2s, asymptote 60% around 10s. Short transcodes
  visibly fill instead of sitting at 0 then snapping to 100.

### Code cleanup
- [ ] **Refactor pass approved V32.19 — file-organization splits only,
  no new abstractions, no behavioural change.** Three independent
  changes; can ship in any order. Onboarding story per change: a new
  dev reads the public header for the class, then jumps to whichever
  `_<area>.cpp` matches their work. Same pattern AppController already
  uses with `_connect.cpp` and `_commands.cpp` (which are partial-
  class files keyed by area, not deep restructurings).

  - [ ] **`AppController_init.cpp`** — extract the 1225-line
    `AppController::initialize()` into per-subsystem `wireXxxSignals()`
    private methods. One method per subsystem (network, dht, nat,
    gossip, storage, router, upnp, firewall, location, power).
    Pattern: each method takes no args, accesses `d->subsystem.get()`
    and `connect()`s its signals. Body of `initialize()` shrinks to
    construction-then-orchestration. Same `AppController_p.hpp`
    include and `SnapClone::Core` namespace as the existing partial
    files. **New-dev story**: "where are the storage signals wired?"
    → grep `wireStorage`, land on a 100-line function.

  - [ ] **`MainWindow_signals.cpp`** — same treatment for the 663-line
    `MainWindow::connectSignals()`. Group by signal source: camera,
    contacts, inbox, settings, wayfinder, picker, swipe, appcontroller.
    Each becomes `connectXxxSignals()`. Pure file-shuffle.

  - [ ] **`WireFormatV3.cpp` / `WireFormatV3.hpp`** — extract the wire-
    format layer (lines ~60–425 of SnapRouter.cpp: constants, helpers,
    `buildV3`, `parseV3`) into `core/WireFormatV3.{hpp,cpp}`. Pure
    functions, no `m_storage` / `m_network` / `m_crypto` dependency.
    Touched every protocol version bump. Defines a
    `Wire::ParsedMessage` struct returned from `parseV3Plus()`;
    `SnapRouter::handleV3Message` becomes ~5 lines of "parse then
    dispatch on flags." **New-dev story**: "what's on the wire?" →
    read one file. Also: opens the door to unit-testing the protocol
    in isolation, and the next bump (tier-aware ANNOUNCE for the
    tiered-delivery work) touches one file instead of three.
- [x] ~~32 inline copies of RFC1918 subnet checks scattered across
  `AppController*.cpp` and `NetworkManager.cpp`. Helper exists
  (`AddressClassification.hpp`) — gradually migrate sites.~~
  **Done V32.32.** Five sites migrated: NetworkManager.cpp:659
  (LAN check on session reuse), AppController_connect.cpp:239
  (transport quality classification), AppController_commands.cpp:158
  + :594 (isPublic gate and live-DHT private-reject), AppController.cpp:970
  (selfAddr public-vs-private). Each was a 3-6 line `isInSubnet`
  chain replaced with one `AddressClass::isPrivate()` call. Net
  ~25 lines removed; single point of truth for RFC1918 set.
  Remaining `isInSubnet` calls in the codebase use runtime
  `QNetworkInterface` ip/prefix entries (interface-subnet checks)
  which is the correct API for that use and not a candidate for
  migration.
- [x] ~~Pollinator vs ContactsPage — PollinatorPage was deleted, but
  some comments still reference the old name~~ **Done V32.32.**
  Audit found only one stale comment in MainWindow.cpp (the
  `kPageFriends /* was kPagePollinator */ removed` comment that
  confusingly suggested kPageFriends itself had been removed).
  Rewrote to clarify history: kPagePollinator + PollinatorPage UI
  were removed; the friends-page constant remains. The
  `PollinatorDiscovery` class is unrelated — it's the network-layer
  multicast-beacon system.
- [x] ~~Several `qCDebug` log sites are at Info level by accident
  (especially in NatTraversal — see the punch/SCUD spam diagnosis)~~
  **Done V32.32.** Four per-packet log sites demoted Info→Debug:
  `NatTraversal::onPunchSocketRead` "SCUD frame from..." (was
  firing per inbound UDP datagram via SCUD wrap), "Received punch
  from..." (per packet during a 60-packet punch volley),
  `UdpChannel::onDatagram` "onDatagram from..." (per inbound UDP
  datagram, **worst offender** — thousands per second during
  video sends), `GossipDiscovery::handleGossipMessage`
  "Gossip-merged peer..." (per peer in every JOIN payload),
  `GossipDiscovery::announcePresence` "JOIN deferred" (common
  startup race). Connection-state-changing logs ("Hole punch
  confirmed", "Late punch from", initial "STUN reflexive")
  remained Info. Default-config users see vastly less log
  noise; `--verbose` still surfaces everything.

### Deferred — bigger refactors (not approved yet, here for memory)
- [ ] `StunClient` extracted from `NatTraversal` (currently 1109 lines).
  Real new abstraction: STUN is its own protocol with its own state,
  punching is a different protocol that just consumes our reflexive
  address. Interaction is one signal. Cost: real type design,
  testability story. Win: 1100-line files reliably hide bugs (the
  V32.19 synchronous-signal-during-abort crash was in this file).
- [ ] `SettingsSection` interface + per-section subfile
  (`src/ui/settings/AppearanceSection.cpp` etc). `SettingsPage` becomes
  a thin shell holding a `QList<SettingsSection *>`. Each section is
  ~150–200 lines, fits in your head. `loadSettings()`/`saveSettings()`
  collapse to a for-loop over sections. Cost: real new interface,
  ~1 focused session. Win: adding a new settings section becomes a
  clear contract instead of "find the right place in 1762 lines."
- [ ] `MediaPipeline` for the CameraWidget recorder/transcoder dance.
  The V32.19 transcoder-clobber bug was a direct symptom of
  CameraWidget knowing too much. A pipeline class owning
  recorder+transcoder+path-bookkeeping would let CameraWidget call
  `pipeline->startRecording()` and listen for `snapReady(path)`. Defer
  until tiered delivery is wired end-to-end — that work will reshape
  this area anyway.

---

### V32.33 — May 14-15 session items

*New items surfaced this session, grouped by where they belong. Promote
into the parent category as worked. Items dated to the session for easy
context recovery; see `/mnt/transcripts/journal.txt` for full reasoning
trails.*

#### Build / infrastructure

- [ ] **Refactor Rust staticlibs into single `snapclone-ffi` crate**
  (workspace layout: ffi-utils rlib + crypto rlib + face rlib + ffi
  staticlib wrapper). Today `snapclone-crypto` and `snapclone-face`
  each produce their own staticlib, each bundling its own copy of
  Rust std/core/alloc. The C++ link sees duplicate definitions of
  thousands of std symbols. Worked around in V32.33 via
  `--allow-multiple-definition` linker flag in `src/CMakeLists.txt`
  (works because the symbols are byte-identical), but the
  architecturally-clean fix is one staticlib. Design decisions
  already made in conversation: four crates, `snapclone-ffi-utils`
  for shared FFI helpers, `snapclone-ffi` as the single staticlib
  the C++ side links. Other staticlib-producing crates (identity,
  proto, saliency, etc.) join the merge only when they get wired
  into the C++ binary. After the merge lands, REMOVE the
  `--allow-multiple-definition` flag.

- [ ] **Verify `LINKER:` cmake flag syntax works across phone
  toolchains.** V32.33 belt-and-suspenders ships both
  `LINKER:--allow-multiple-definition` and raw
  `-Wl,--allow-multiple-definition`. The original
  `if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")` guard was
  silently dropping the flag on the phone (compiler ID didn't
  match the regex); the guard was removed. If similar
  CMake-version-or-compiler-detection issues bite later, look
  here first.

- [ ] **`cargo` unavailable in scratchpad environment.** Multiple
  iteration cycles this session because Rust-side changes can't
  be `cargo check`ed before shipping. When making Rust changes,
  prefer conservative idioms (`Vec<f32>` over `[f32; N]`,
  Serialize-derived types, etc.) over ones that *should* work but
  aren't verified.

- [ ] **`forbid(unsafe_code)` vs `deny(unsafe_code)` lesson.** Any
  Rust crate that exposes FFI must use `deny` at crate root (so
  the FFI module can opt out with `allow`), not `forbid` (which
  is final). `snapclone-face/src/lib.rs` was already migrated to
  `deny`; check any new FFI-exposing crate for this.

#### Camera

- [ ] **Rear camera shows front camera on phone.** Per logs,
  `availableDevices: pw-dump found QList()` consistently —
  the gst-launch backend is finding no rear-cam virtual node.
  Camera-bridge logs spamming `Failed to parse output pipeline
  for virtual-cam-rear/front/tracking` every ~5s suggest the
  bridge is also failing to create vcams. Two suspects: bridge
  pipeline syntax error (changed recently?) or PipeWire node
  visibility from app's user. Diagnostic turn needed.

- [ ] **Camera-bridge spam loop.** `bridge: ... Failed to parse
  output pipeline` repeats every 5s indefinitely on the phone.
  Need backoff or a circuit breaker — currently it just
  retries forever. Cosmetic but pollutes logs.

- [ ] **Camera crashed once on mobile** during a snap session.
  No reproduction trigger yet. If it recurs, grab the lines
  just before in the log; that's what'll narrow it down.

- [ ] **PIP v1 (shipped V32.33) is laptop-only.** Selector entry
  "Rear + Selfie PIP" only appears when `availableDevices()`
  reports both Facing::Rear AND Facing::Front. On phone it
  won't appear until the rear-cam issue above is resolved.

#### PIP follow-ups

- [ ] **Drag-to-reposition PIP overlay.** v1 is fixed
  bottom-right. The earlier "PIP composite + drag/swipe-up-lock
  gesture" deferred item from V32.32 transcript covers this.
  Touch tracking with snap-to-corner anchors is the obvious
  shape.

- [ ] **Tap-on-PIP behavior.** v1 is a no-op
  (`WA_TransparentForMouseEvents`). Likely candidates: swap
  main/PIP, or hide-PIP. Defer until we know what users want.

- [ ] **Two-stream capture wire format.** v1 captures only the
  main camera; PIP is preview-only. The user-requested design
  is to capture both streams as separate assets and composite
  at view time, which unlocks (a) parallel face-detection +
  transcode pipelines, (b) recipient-side PIP repositioning,
  (c) sending as two video snaps. Requires wire format
  extension (multi-asset snap), composer changes, viewer
  changes. Multi-turn architectural piece.

- [ ] **Frame rate competition between two cameras.** Two
  QtCameraBackend instances may each run slower than one
  alone due to camera bus contention. If main viewfinder
  drops below ~20fps in PIP mode, address.

- [ ] **Audio in video recording + PIP mode.** v1 PIP is
  image-only. Recording while in PIP mode is currently
  undefined — needs design.

#### Network / DHT

- [ ] **Recency-window pre-warm of contact connections.**
  Discussed at length. Pre-warm DHT lookup + STUN for contacts
  with recent activity (recent send/receipt, connection up in
  last N minutes). Self-gating — no statistical-noise problem
  of the histogram approach we initially considered.
  Cheap latency win; opt-in.

- [ ] **Per-contact salted DHT keys** (per `docs/DHT_DISCOVERABILITY.md`,
  shipped V32.33). Implementation pending. Salt rotation on
  block, four bootstrap paths (in-person/mutual-contact/
  fingerprint/public-opt-in).

- [ ] **BLE highways design doc.** Queued from this session.
  TOTP-style rotating identifiers via shared secret, advertised
  in BLE service data (not MAC — OS restricts MAC access).
  Substantial work (BLE protocol + cross-platform permissions
  + scan duty cycling + sync state machine) but a real
  proximity-aware presence layer. Design doc first, then scope.

- [ ] **Partner-pair persistent address cache** (Level 1 from
  partner-pair discussion). Cache (ip, port, last_seen_ts) per
  contact; try cached before DHT lookup. Should ship for ALL
  contacts, not just partner pairs. Simple, no privacy cost,
  saves the 1-5s DHT lookup when the address is fresh.

- [ ] **Blocking design** (per BLOCKING.md, not yet written).
  Client-side enforcement only (DHT is dumb storage). Blocklist
  + pre-handshake reject + mailbox refusal + per-contact key
  rotation. Block-evasion heuristics opt-in. Resist temptation
  to change announcement patterns based on who blocked whom
  (costs other contacts).

- [ ] **Duplicate snap reception chatter.** Logs show same snap
  hash received 3-4 times per delivery, deduped correctly each
  time. Working as intended but suggests SYNC frequency could
  be tuned down. Minor perf issue, low priority.

#### UI

- [ ] **Storage section live-refresh.** V32.33 storage section
  refreshes on settings-page entry and after clear. Doesn't
  poll. If user records a video while looking at settings,
  the total won't update until they leave + re-enter or hit
  clear. Probably fine; flag if it bothers anyone.

- [ ] **Storage section per-category breakdown.** v1 is total
  only. If total ever feels too coarse, `snapcloneDataDirs()`
  already returns the list and `formatBytes()` is reusable
  — wiring per-category is straightforward.

- [ ] **Storage clear — granular categories.** v1 clears all
  of CacheLocation. If users want "clear recordings older
  than 7 days" or "clear failed transcode intermediates only",
  build on the same infrastructure.

- [ ] **Face library wipe confirmation dialog.** Previously
  rationalized as "no confirm needed because library is empty
  at start." Now that the V32.32 capture path actually
  populates it, the rationale's gone. Add a confirm.

- [ ] **ModalSurface L4 tap-outside-to-dismiss** stuck on
  contact editing flow. Reported earlier session. Awaiting
  reproduction trace.

#### Video / playback

- [ ] **Playback slow on postmarketOS.** Two-tier fix discussed:
  Tier 1 = factor-2 downscale fallback (auto-detected on
  aarch64 with no hardware decode); Tier 2 = drop the JPEG
  re-encode entirely, use a proper video sink
  (`pipewiresink`, `qmlglsink`, or Qt native multimedia).
  Ship Tier 1 first as quick win.

- [ ] **`gst-svtav1enc` transcode failure on phone 1.**
  `streaming stopped, reason not-negotiated (-4)` — falls
  back to sending raw .mkv (which works) but the AV1 path
  is broken. Could be a caps negotiation issue between
  decodebin and svtav1enc.

- [ ] **`gst-discoverer` / `gst-device-monitor` async probe.**
  Both currently block briefly on probe. Filed previously,
  low priority.

#### Behavioral inference / privacy

- [ ] **Inference symmetry principle.** If feature X infers
  about contact Y, contact Y should be able to see that
  inference exists. Codify this when designing presence /
  pre-warm / availability features. From "tracking as
  fighting" discussion.

- [ ] **Avoid building relationship-graph datasets**
  (per-contact histograms) without explicit opt-in. Per-contact
  send-time histogram idea was scrapped in favor of
  recency-window pre-warming for this reason.

#### Rust audit (separate turn)

- [ ] **Survey codebase for Rust-rewrite candidates.** Discussed
  but not started. Survey hot paths, safety-critical code,
  memory-allocation-heavy regions, and places that duplicate
  Rust we already have. Output: ranked list with effort
  estimates. Should land AFTER the `snapclone-ffi` merge
  (above) so candidates can be assigned to the right crate.

#### Security review findings (V32.33)

*All findings from `docs/CRYPTO_REVIEW.md` — see that doc for the
full rationale, code references, and suggested fixes. Listed here
by severity for prioritization. Findings are NUMBERED with their
S-N tag from CRYPTO_REVIEW.md.*

**Serious (fix before any production / wider deployment):**

- [ ] **S-1: Default build uses unauthenticated placeholder crypto.**
  Without `-DSNAPCLONE_RUST_CRYPTO=ON`, the Qt placeholder ships.
  Its "session key" is computable from public keys alone. Fix:
  invert default — require explicit opt-out for the placeholder.
  `src/core/AppController.cpp:55-61`.

- [ ] **S-2: SHAK handshake has no signature / proof-of-possession.**
  Attacker can spoof identity (can't decrypt replies but causes
  contact-confusion, transport hijack, contacts auto-saved as
  Known under attacker-chosen displayName). Fix: sign SHAK
  payload with Ed25519. `src/core/AppController_connect.cpp:14-156`.

- [ ] **S-3: AEAD ciphertext not bound to sender/recipient/seq via AAD.**
  No replay protection at crypto layer. The application's dedup
  cache is in-memory only and storage write happens BEFORE dedup
  check. Fix: pass `(sender || recipient || seq)` as AAD to
  encrypt/decrypt. `rust/snapclone-crypto/src/lib.rs:184`,
  `src/core/SnapRouter.cpp:470-490, 683-733`.

- [ ] **S-4: sc-ID is 32 bits — adversarial collision feasible on
  one laptop.** Foundational identity weak. Fix: 128-bit
  minimum, ideally derived from pubkey hash.
  `rust/snapclone-identity/src/lib.rs:11-13`.

- [x] ~~**S-5: MailboxId truncated to 64 bits — targeted collision
  feasible.**~~ **Done V32.34+ (mailbox overhaul).** `MailboxId::
  for_recipient` now returns full SHA-256 (64 hex / 256-bit) instead of
  the truncated 16-hex/64-bit form. A `for_recipient_legacy_64bit` shim
  remains for a transition window so peers can poll both buckets while
  upgrading; remove it once the deprecation window closes.
  `rust/snapclone-mailbox/src/entry.rs`. NOTE: this is the Rust
  `snapclone-mailbox` crate, which is `rlib`-only and **not yet wired
  into C++** — the live connection path uses the separate connreq
  mailbox in `JamiAdapter_dht_impl.cpp` (whose key derives from
  OpenDHT's infohash of `sc_id + "/connreq"`; salting that key is the
  live-path analog of this fix and still TODO, needs per-contact salt).

- [ ] **S-19: DHT mailbox entries are plaintext metadata broadcast.**
  Not crypto-brute-forceable — there's no crypto to break.
  Mailbox entry is plaintext JSON containing sender, recipient,
  size, timing, and the sender's IP (in delivery_hints).
  Anyone scraping the DHT learns the social graph. The 512-byte
  size cap is irrelevant to the leak — it's a privacy disaster,
  not a brute-force one. Fix: encrypt the entry to the recipient,
  ideally with hybrid PQ. Composes with the per-contact salted-key
  DHT_DISCOVERABILITY work. `rust/snapclone-mailbox/src/entry.rs`.

- [ ] **S-20: No TOFU check on peer pubkey changes — silent
  replacement.** `savePeerPublicKey` uses
  `ON CONFLICT DO UPDATE`, so every SHAK overwrites the stored
  pubkey for that peer with no comparison. Combined with S-2
  (unsigned SHAK), this makes persistent MITM trivial: Mallory
  sends SHAK with their pubkey claiming to be Alice at any time,
  Bob's stored Alice-pubkey silently becomes Mallory's. From that
  moment, encrypts-to-Alice = encrypts-to-Mallory. Fix: compare
  incoming pubkey to stored one BEFORE saving; on mismatch,
  surface as user-confirmable key-change event.
  `src/storage/StorageManager.cpp:1337-1352`.

**High:**

- [x] ~~**S-6: Envelope padding is structured, not random** despite
  comment claiming compression-attack defense.~~ **Done V32.34+
  (mailbox overhaul).** Replaced the `DefaultHasher`-seeded-by-
  `SystemTime` ramp with `rand_core::OsRng` CSPRNG fill (OsRng reached
  via the rand_core getrandom feature, pinned to the 0.2.x line to
  avoid the edition2024 0.4.x). Added a `padding_is_nondeterministic`
  test confirming two packs of identical payload differ in the padding
  region. `rust/snapclone-mailbox/src/envelope.rs`. Same wiring caveat
  as S-5: the crate isn't consumed by C++ yet.

**Medium:**

- [ ] **S-7: Private key bytes in non-zeroizing C++ heap.**
  `RustCryptoService::loadPrivKey` returns QByteArray. Fix:
  small RAII zeroizing wrapper. `src/crypto/RustCryptoService.cpp:77-82`.

- [ ] **S-8: Session keys in QHash<QString, QByteArray>, no zeroize.**
  Long-lived process holds every contact's session key in
  cleartext. Fix: zeroizing values + key rotation.
  `src/crypto/RustCryptoService.cpp:167`.

- [ ] **S-9: No session-key rotation / no forward secrecy.**
  Single ECDH-derived key reused forever per peer-pair. One key
  compromise = total transcript recovery. Contradicts the
  ephemeral-snap user promise. Fix: ephemeral DH per SHAK at
  minimum; Double Ratchet for full FS.

- [ ] **S-10: No small-subgroup check on peer pubkey.** Attacker
  sends zero or low-order pubkey, derived "session key" is
  predictable; receiver encrypts to it; attacker decrypts. Fix:
  reject low-order points before ECDH.
  `rust/snapclone-crypto/src/lib.rs:135-140`.

- [ ] **S-11: Ed25519 verify is not strict** (signature
  malleability). Fix: use `verify_strict`.
  `rust/snapclone-crypto/src/lib.rs:304-309`.

- [x] ~~**S-12: MailboxEntry in DHT is unsigned** — sender_id self-
  asserted.~~ **Done V32.34+ at the crate level (mailbox overhaul).**
  New `SignedMailboxEntry` wraps the entry with an Ed25519 signature
  over its canonical bytes plus the sender's embedded verifying key.
  `verify_self_consistent()` uses `verify_strict` (rejects malleability,
  mirrors S-11); `verify_with_key()` is the proper check once the caller
  has the sender's key from the contact store (rejects an attacker who
  signs with their own key but claims another sender_id). Tests cover
  roundtrip, tamper rejection, and wrong-key rejection. Depends on
  per-contact pubkey infra to be USEFUL end-to-end (the caller must map
  sender_id → known key); that mapping + wiring the crate into C++ is
  the remaining work. `rust/snapclone-mailbox/src/entry.rs`. NOTE: the
  live connreq mailbox already gets implicit sender auth via AEAD (a
  forger can't produce valid ciphertext without the shared key), so the
  live path's forgery exposure is lower than the bare-entry crate was.

- [ ] **S-13: `try_into().unwrap()` panics across FFI = UB.**
  Replace with explicit length checks returning InvalidInput.
  `rust/snapclone-crypto/src/lib.rs` — multiple sites.

**Low:**

- [ ] **S-14: HKDF without salt** — fine today (X25519 secret is
  high-entropy) but forecloses future per-channel salting. Flag
  for awareness.

- [ ] **S-15: Identity keys dir doesn't enforce 0700 on parent.**
  Other local users on a multi-user device can `ls` and see which
  sc-IDs exist. Fix: explicit chmod on the keys/ dir.

- [ ] **S-16: `extract_sc_id` matches FIRST `(sc-` not the suffix.**
  Hostile display name can confuse extraction. Fix: anchor to
  end. `rust/snapclone-identity/src/lib.rs:25-30` AND
  `src/core/AppController_connect.cpp:65-71` (same logic in C++).

- [ ] **S-17: Envelope padding seed uses SystemTime — fails in
  weird clock states.** Subsumed by S-6 (fix the whole padding
  approach).

- [ ] **S-18: CryptoError enum not marked non-exhaustive at FFI.**
  Future variants silently misinterpreted by stale C++ headers.
  Low risk, just convention.

**Cross-cutting themes:**

- Identity is the weakest layer (S-2/4/5/12/16). A single
  coordinated identity rework is the highest-value security fix.
- AEAD context-binding (S-3) is cheap and unlocks replay defense.
- C++ memory hygiene around keys (S-7/8) is a small RAII wrapper.
- Defaults (S-1) is a one-line CMakeLists invert with big impact.

**Not in this audit (filed for a future audit):**

- Side-channel timing analysis.
- `cargo audit` of dependencies (couldn't run; rerun before
  production).
- Fuzz testing the wire-format parsers (`handleV3Message` etc.) —
  these accept attacker bytes and bugs there bypass crypto.
- ProVerif / Tamarin protocol correctness.
- Constant-time analysis of compiled code (not just source claims).
- Memory inspection after compiler optimization.

### Camera framework hardening & passthrough

**Context (V32.34+):** the existing camera stack was bootstrapped from the
Rust `camera-bridge` testbed binary — useful as a reference for what
GStreamer pipelines work on FP5, but the C++ side has accumulated
hard-coded timeouts and one-off paths instead of a clean policy/probe/
tester architecture. The framework work below replaces those with
named, configurable, testable abstractions.

- [x] **CameraPolicy:** named timeouts and limits (startup, first-frame,
  early-exit, capture caps, rotation) instead of constants buried in
  GstLaunchBackend. Filed V32.34+; see `include/ui/camera/CameraPolicy.hpp`.
- [x] **cam --list integration** — authoritative facing classification
  via libcamera's own tool, with suffix-safe path→node resolution.
  Replaces the i2c-address-sort heuristic. V32.34+.
- [x] **Error translation from gst-launch stderr** — match common
  GStreamer error patterns (negotiation, busy, permissions, missing
  element) and surface user-actionable hints instead of generic
  "Resource error". V32.34+.
- [x] **First-frame watchdog** — detect "pipeline running but no
  frames" (stuck libcamera kernel driver) within 5s and trigger
  alternate-node retry. V32.34+.
- [ ] **CameraProbe:** non-destructive node tester. Opens a node,
  negotiates format, pulls one frame, closes. Reports available
  formats + resolutions WITHOUT committing the node to a session.
  Powers "Test camera" button in Settings and an automated startup
  health check (run once after device enumeration, cache results).
- [ ] **CameraTester UI surface** — Settings panel with per-node
  Status column (Working / Untested / Failed / Format-conflict),
  individual Test buttons, full diagnostic dump for bug reports.
- [ ] **In-process GStreamer backend** — replace gst-launch fork+pipe
  with linked gstreamer-1.0 + gstreamer-app-1.0. Gives us:
   * Bus access for real error messages (not stderr scraping)
   * No buffer-copy through pipe
   * State-readiness check (set_state(Playing) + wait_state) like
     `camera-bridge/src/capture.rs`
   * Faster camera switches (no fork+exec)
  Cost: build deps grow, GObject ref-counting from C++. Defer until
  the policy/probe/tester layer above is stable.

- [ ] **Camera CPU→GPU colorspace offload — PARTIAL (V32.34+).** The
  YUV→RGB conversion + downscale was pinning the CPU on the gst-launch
  path, which capped sustainable fps and forced low capture resolution.
  **Done so far:** `GstLaunchBackend::buildConvertScaleSegment()` now
  detects (cached `gst-inspect-1.0` probe) whether `glupload /
  glcolorconvert / glcolorscale / gldownload` are installed and, when
  `policy.preferGpuConvert` is set, runs the conversion + scale on the
  Adreno GPU instead of the CPU `videoconvert ! videoscale` path. Falls
  back automatically to the deterministic dual-`videoconvert` CPU path
  when GL elements are missing. Also added stride-alignment of the output
  width (multiple of `strideAlignPx`, default 4) to kill the
  resolution-dependent green/red tint caused by our `width×4` QImage
  reader drifting when GStreamer pads rows — the tint that "doesn't
  survive transcode" because transcode re-reads via stride-aware video
  meta. **Still TODO:**
    * **Verify on device which path runs** — logs say `Using GPU
      colorspace path` vs `Using CPU colorspace path` and
      `gst element glupload available/NOT`. Confirm GL path actually
      engages on FP5 / pmOS Plasma Mobile, and that the tint is gone
      with it. If tint persists on the GPU path, `gldownload`'s output
      caps may be lying about format on that build — pin harder.
    * **`gpuSourceWidth/Height`** policy fields exist (off by default)
      to request a HIGHER source resolution when the GPU path is active
      (the expensive conversion is no longer on the CPU, so we can
      afford it). Needs device validation before enabling by default.
    * **DMA-BUF zero-copy — the bigger win, NOT yet done.** `gldownload`
      still copies GPU→system memory so we can write raw bytes to the
      fd pipe. True zero-copy needs the in-process backend above:
      keep the frame as a GL texture / DMA-BUF all the way to the
      Qt render path (QRhi / QSGTexture import of the DMA-BUF fd),
      never touching system memory. That removes the per-frame
      3.7MB (1280×720×4) memcpy entirely and is what unlocks 1080p+
      at 30fps. Blocked on: in-process GStreamer backend (above) +
      a Qt RHI texture-import path. Large change; design brief:
      gst `appsink` with `caps=video/x-raw(memory:GLMemory)` →
      pull `GstGLMemory` → export DMA-BUF fd via `gst_gl_memory_*`
      → import into Qt via `QRhiTexture::createFrom` (Qt 6.6+) or a
      custom QSGTexture. Per-frame cost drops from "decode + 2 copies
      + scale" to "GPU fence wait + texture bind".
    * **vaapi / v4l2 hardware converter alternative** — some pmOS
      builds expose `v4l2convert` (hardware m2m colorspace converter)
      which is even cheaper than GL for pure format conversion (no
      GL context setup). Add it to the `buildConvertScaleSegment`
      preference order ahead of GL when present.

#### Camera passthrough to other apps

**Goal:** other apps on the device (Firefox WebRTC, Signal video calls,
Zoom, etc.) should be able to consume SnapClone's chosen camera node
WITHOUT each app re-implementing the FP5-libcamera dance.

Two layers to consider:

- [ ] **PipeWire republish:** SnapClone (or a paired daemon) publishes
  a `node.name = snapclone-camera` PipeWire node fed by whatever
  the user selected in our dropdown. Other apps see it as a generic
  webcam. Requires gst-pipewire-sink with a fixed node.name in the
  pipeline — already how camera-bridge publishes virtual-cam-*, so
  the implementation exists; just needs lifting into SnapClone's
  own backend rather than relying on the bridge being installed.
  *This is the practical short-term win — many apps that use PipeWire
  for webcam input already work transparently.*
- [ ] **v4l2loopback fallback:** for apps that can't be coaxed to use
  PipeWire (older Skype, some game engines), expose the chosen camera
  as `/dev/videoN` via v4l2loopback. Same pipeline tail-end as the
  PipeWire republish, just a different sink. Requires v4l2loopback
  kernel module on the user's system — package as `apk add v4l2loopback`
  on pmOS.
- [ ] **Permissions model:** when SnapClone is the camera owner, other
  apps consuming it should see the same blur/freeze/private-mode
  setting we expose to the user. If the user toggles "camera off" in
  SnapClone, downstream apps should see a black frame, not get a
  raw feed. Implement by switching the source-side of the republish
  pipeline rather than tearing it down (apps that rely on persistent
  /dev/video stay open).
- [ ] **Discovery for the user:** "Apps using SnapClone's camera"
  panel in Settings — lists current PipeWire consumers of our
  published node, can revoke individually.

**Risks / unknowns:**
- Republishing while we're also reading creates two consumers of the
  underlying libcamera node. PipeWire handles this fine in theory
  (the libcamera node has multiple downstream subscribers); in
  practice FP5's libcamera-gstreamer plugin sometimes refuses a
  second client. Needs empirical testing.
- Some carriers / Android-side enterprise policies block /dev/video
  use. Not our problem to solve, but document it for users.
- v4l2loopback support is patchy across kernels. The pmOS kernel
  config would need verification (`zcat /proc/config.gz | grep V4L2_LOOPBACK`).

**Filed:** V32.34+, prompted by user noting the existing camera-view
was a testbed. The republish path is the first thing to prototype.

---

## 🟢 Polish

- [x] ~~Loading skeleton state for contacts page first-render (currently
  pops in)~~ **Done V32.32.** `m_initialLoadDone` flag distinguishes
  "never been told the contacts" from "list is empty." Before the
  first `setContacts()`, 4 grey rounded-rectangle skeleton cards
  appear (avatar disc + two text bars each), then real cards replace
  them. The "No contacts yet" empty-state label only fires AFTER
  initial load when the list is genuinely empty.
- [ ] Wayfinder map render uses `update()` not a proper repaint region;
  optimise for high-contact counts (>50)

- [ ] **V32.31 landmarks — foundation shipped, follow-ups remain.**
  V32.31 lands the local data model: `LocationSource` enum, `Landmark`
  struct, `landmarks` storage table (migration 14), CRUD methods,
  `LandmarkHash::{blurHash, edgeHash}` (blur-before-Sobel for privacy),
  `AddLandmarkDialog` modal, "Add landmark" button in Contacts header.
  All landmarks default to `LandmarkScope::Private` — NO DHT publish
  path runs in V32.31.

  **Phase 2 (post-privacy-review):**
    - DHT publish path for `LandmarkScope::Public` landmarks.
      134-byte wire format on the DHT (pubkey 32 + lat 8 + lon 8 +
      acc 2 + pHash 8 + edgeHash 4 + ssidHash 8 + sig 64). Display
      name + photo stay LOCAL on the publisher — fetched p2p if
      recipient wants to see.
    - Re-publish economy: recipients who resolve against a landmark
      cache it locally AND re-publish for their own ~10min window.
      BitTorrent-swarm-for-facts model.
    - Geohash7 column populated (~76m cell precision) for DHT lookup
      indexing.
    - Ed25519 sign-on-publish using identity key.
    - Per-publisher rate-limit + impossible-movement heuristic for
      bad-actor detection. Local reputation table; no global mod.
    - Scope picker UI in AddLandmarkDialog (Private / Contacts / Public)
      with explicit warning + confirm on Public.
    - Web-of-trust filter at landmark-display: default "from my
      contacts or contacts-of-contacts." Anonymous public landmarks
      get a separate "unverified" tab.

  **Other landmark follow-ups:**
    - WiFi BSSID capture at landmark-creation time. NetworkManager
      DBus query: `nmcli device wifi list active` or equivalent.
      Hash bssid → 8 bytes, store on landmark row. Then a
      LocationResolver can match "I'm on this BSSID → I'm probably
      at this landmark."
    - Photo capture path: either "take photo now" (camera quick
      capture) or "use a recent snap" (pick from inbox). Compute
      LandmarkHash and store the hashes on the landmark row. Photo
      itself is NEVER stored or shared.
    - "My landmarks" list page or section on Wayfinder showing all
      stored landmarks with delete affordance.

- [ ] **V32.32 — landmark candidate scoring + viewfinder hint.**
  When the user is composing a snap, a cheap analyzer runs over the
  preview frame to score "is this a good landmark candidate" ∈ [0,1].
  Heuristics computed from the existing Sobel pass:
    - Strong horizontal edge (mountain/skyline/horizon) → column-sum
      peak
    - Edge-density variance across frame thirds → distinctive subject
      against neutral surroundings
    - Vertical-edge dominance → architectural feature (street sign,
      doorway, building)
    - Low high-frequency-noise to total-edge-energy ratio → clean
      structure (not a crowd, screen, or LED show)

  When the score crosses a threshold (~0.7), a subtle "+ Save as
  landmark" affordance fades into the viewfinder corner.

  UX guardrails:
    - Cooldown: dismissed hint suppresses re-show for the same scene
      fingerprint (photoBlurHash) for at least 1 hour.
    - Only fire on convincingly-good scores, not borderline — to
      avoid training users to ignore the hint.
    - Hint must not block shutter; pure visual overlay.

- [ ] **V32.32 — iio-sensor-proxy compass + wayfinder hands-off mode.**
  Desktop linux exposes a compass via `net.hadess.SensorProxy` on
  the system DBus. Phones via QtSensors. New `CompassSource` QObject
  that emits headingChanged(degrees).

  Wayfinder gains three modes, toggled by tapping the center self-dot:
    1. North-up (current default)
    2. Track-up — screen rotates so heading = up; the self-dot stays
       fixed at the center pointing up
    3. Fixed-heading — locks rotation to whatever heading was active
       when entered (useful for stop-and-check-map situations)

  Tap-center-to-cycle handled by PressGesture (V32.30) — already
  reusable. Mode persists across sessions in QSettings.

- [ ] **V32.32 — vehicle / bike mode (passive location pipeline).**
  Stay useful when the user CAN'T look at the screen. Pieces:
    - Auto-zoom by velocity: consecutive GPS fixes → speed estimate
      → expand Wayfinder zoom radius so the view is "5-10 min ahead"
      not "200m around me."
    - `LocationSourceListener` QObject: unified "location signal
      changed" event from any of {GPS, NetworkManager WiFi
      association, manual override change}. Subsystems subscribe to
      this one signal instead of polling each source.
    - Haptic alerts on transitions (entering a known landmark area,
      contact appears on wayfinder, etc.) gated behind a
      `feedback/hapticAlerts` setting that defaults OFF. Linux
      desktop has no good haptic surface — phone-only feature for
      now. Use Qt's QtFeedback or platform-native bindings.

- [x] ~~Settings page: surface "your DHT id" as a QR code for easier
  pairing~~ **Already done before V32.32 (TODO entry was stale).**
  `SettingsPage::setNodeId` populates `m_qrLabel` with a `makeIdQr()`
  pixmap; tapping it calls `showExpandedQr()` for a full-screen view.
  QR encoder lives in `QrCodeRenderer.hpp/cpp`; the matching scanner
  is in `QrScanDialog`. Both use a custom (non-standard) module
  pattern that works between SnapClone endpoints only.
- [ ] Help / first-run tutorial overlay
- [ ] Per-contact themes (each friend gets a colour accent)
- [ ] **Dynamic themes — V1 (Modes A + B) SHIPPED V32.29-V32.30;
  refinements remain.** Mode A "Ambient: Inbox" derives palette from
  recent inbox photos on pick. Mode B "Ambient: Camera" (Content
  Creator Mode) subscribes to the live viewfinder at 1Hz and re-derives
  continuously while the camera page is visible. V32.30 also lands
  the AmbientBlobs overlay (pulsing radial-gradient splotches in
  derived accent colors) + readability-safe BG tinting (saturation
  capped at 0.18, lightness anchored to base palette). Synthetic
  palettes are session-only and mutually exclusive. See WORK_LOG
  V32.29 + V32.30 for the full pipeline design.

  **Still open (refinements, not blockers):**
    - [x] ~~**Auto-refresh on inbox-update** for Mode A. Currently one-shot
      on pick. Need to subscribe to inbox-update + 30s throttle.~~
      **Done V32.30 wave 3.** `AmbientInboxDriver` subscribes to
      `AppController::snapReceived`, re-derives on each snap arrival,
      throttled to one derivation per 30s. Initial derivation on start.
      Mutually exclusive with camera mode via paired stop signals.
    - [x] ~~**Temporal smoothing for Mode B.** Walking the camera produces
      visible palette jitter; 5-10s exponential moving average over
      derived accents would calm it. Less urgent than expected since
      the 0.18 sat cap already smooths the visual.~~ **Done V32.30
      wave 3.** Split deriver into `deriveColors`/`fromColors`;
      `AmbientCameraDriver` now applies α=0.15 EMA in HSL space (with
      shortest-arc hue lerp for clean green↔magenta transitions).
      ~4.3s half-life. First-tick seeds directly without blend.
    - **k-means refinement of accent selection.** V1 uses saturation-
      bucket averaging which collapses multi-color sources to a muddy
      mid-color. Real k-means (k=3 in HSL, pick brightest non-grey
      cluster) is the originally-specified algorithm.
    - **Persistence option.** Synthetic doesn't survive app restart
      by design; some users may want a "Stay in Ambient mode" toggle.
    - **Snap-nearest-palette vs full synthetic** — original spec
      considered interpolating between the closest two named palettes
      to preserve all 24 slots' relationships. V1 only overrides
      9 slots (BG/ACCENT/GRAD/GLOW), so partly addressed. Full
      interpolation is a v3 idea.
    - **AmbientBlobs per-page placement.** Currently a single
      window-wide overlay. Wayfinder might benefit from blobs scaled
      to its radar-canvas dimensions and bias-placed toward populated
      quadrants.

---

## 🐛 Bugs & gotchas surfaced from log review (V32.33+)

- [ ] **🔴 MID-SESSION segfault on M2 Ultra after swiping (NEW, V32.34+,
  NARROWED — defensive fix applied, NOT confirmed root cause).**
  V32.34+ UPDATE: user reports it is **always reproducible on swiping LEFT
  toward / landing on the Wayfinder page.** That narrows it sharply.
  Strongest candidate found by inspection: `WayfinderPage::event()`
  intercepted EVERY TouchEnd/TouchCancel delivered to the page and
  synthesized a fake `mouseReleaseEvent` from it — including the TouchEnd
  that is just the TAIL of a page SWIPE landing on Wayfinder. That fake
  release ran the node hit-test (and the swipe/gesture machinery, incl. the
  new unified-nav commit path) during the slide-in, a plausible
  use-after-free / re-entrancy window. DEFENSIVE FIX V32.34+: track the
  TouchBegin position + whether the touch began ON this page, and only
  synthesize the tap if the touch began here AND travelled ≤16px (a tap);
  a swipe travels far and now propagates normally to SwipeNavigator instead
  of being turned into a fake click. This is ALSO correct behavior
  regardless of the crash (a swipe should never register as a node tap).
  BUT: this is a TARGETED GUESS at the trigger, NOT a proven root cause —
  the crash could still be elsewhere (the DHT worker-thread path, the
  unified-nav gesture commit). Only the backtrace settles it. If it still
  crashes after this fix, get `coredumpctl gdb` → `bt` on the M2 Ultra;
  also A/B with `SC_UNIFIED_NAV=0` (does the new gesture code matter?) and
  note whether it reproduces landing on Wayfinder via the TAB BAR tap
  (no swipe) vs. only via swipe — that isolates touch-handling from
  page-landing.

  ORIGINAL ANALYSIS (pre-narrowing):
  fires DURING the session, ~13s after the camera failed, at the t+30s DHT
  mailbox check, following a swipe. Timeline from the log:
    1. Camera gst backend can't start (this M2 Ultra dev box has no real
       pipewire camera node): `virtual-cam-rear` → exit 255 "target not
       found" → auto-fallback swap to `virtual-cam-front` → also exit 255.
       (Also visible: `Xlib: extension "DRI2" missing` — benign on this
       headless-ish display.)
    2. `stop() called — was-active: false, frames-seen: 0` (camera gave up).
    3. appActive false→true (the swipe / focus change).
    4. t+30s `[MAILBOX] Startup check for connection requests` →
       `Checking DHT mailbox` → **segfault (core dumped)**.
  SUSPECTS (unproven — I did NOT guess-patch):
    * The DHT mailbox path (`checkConnectionRequests` → `decryptPayload` →
      `deriveEndpointKey`) runs on opendht WORKER threads. I recently added
      a mutex-guarded memo to `deriveEndpointKey`. Re-reviewed it: static
      local init is thread-safe (C++11+), QByteArray copy-return under lock
      is safe, cacheable inputs are only short scIds (no thrash). I could
      NOT find a bug in it by inspection — but it IS the newest thing in
      this exact path, so it can't be cleared without the backtrace.
    * The camera double-teardown (finished-handler early-exit fallback +
      a swipe-driven hideEvent stop()) running near-concurrent with DHT
      worker-thread callbacks. The retry uses singleShot(100) so it doesn't
      re-enter, and stop() nulls m_proc first — looked correct.
  DEFINITIVE NEXT STEP: the core was dumped. On the M2 Ultra:
  `coredumpctl gdb` (or `lldb -c <core> ./SnapClone`) then `bt` — that
  names the exact crashing frame and settles which suspect (if either) it
  is. Without it I'd be guess-patching working code. Also useful: does it
  reproduce (a) with a REAL camera present, (b) without swiping, (c) with
  `SNAPCLONE_RUST_CRYPTO=OFF` to A/B the crypto path? Each isolates a
  variable.


- [x] ~~**DHT adapter recomputed the endpoint key (full HKDF) on every
  announce + every inbound value.**~~ **Fixed V32.34+ (efficiency).**
  Insight (user): OpenDHT itself hasn't had the AI scrutiny the rest of
  the tree has — but the *adapter* we own (`JamiAdapter_dht_impl.cpp`) is
  fair game. `deriveEndpointKey(scId)` runs a full HKDF (4 SHA-256 hashes
  + two 64-byte XOR loops) and is called for the SAME constant self-scId
  on every announce (timer + network-change + every connection-request
  publish) AND on every inbound DHT value during mailbox checks (the
  decrypt path) — recomputing an identical 32-byte result each time. Added
  a small thread-safe memo (function-local static, mutex-guarded because
  opendht callbacks fire on worker threads). Subtlety handled: the same
  helper is ALSO (mis)used as a MAC over per-message data (the tag path
  passes hex(key‖nonce‖ct), unique per message). To stop those unique
  inputs from evicting the valuable constant-scId entry, the cache only
  stores SHORT inputs (≤128 chars — an scId is a handful of chars; the
  tag-hex is always far longer), so the tag path computes fresh and never
  touches the cache while the self-key stays resident. The crypto BYTES
  produced are byte-identical (pure-function memo) — no security change.
  NOTE: I deliberately did NOT touch the crypto CONSTRUCTION itself (the
  tag = deriveEndpointKey(hex(...)) homegrown MAC, the SHA-256-CTR
  keystream) — that's a security-sensitive design, not an efficiency
  matter, and changing it blind would risk weakening it. Flagged for a
  dedicated crypto-review pass if desired, separate from efficiency.
  FOLLOW-UPS NOTED (not changed — need device/behavior validation):
    * The `networkChangeTimer` polls `collectLocalIps()` every 2 min, 24/7,
      even screen-off — a wakeup every 2 min for the whole session. Could
      gate on SleepState (skip while ScreenOff/parked) or drive off the
      app-wide QNetworkInformation reachability signal instead of polling.
      Interval is a reachability tradeoff (too sparse = slow to notice a
      WiFi→cellular switch), so tune on-device rather than guess. Pairs
      naturally with the DHT-park-on-screen-off work.


- [x] ~~**Inbox reload did an N+1 over storage (loaded every snap of
  every thread on each refresh).**~~ **Fixed V32.34+ (efficiency).**
  `InboxWidget::reloadConversationsFromStorage()` ran one
  `loadConversationSnaps` PER conversation — loading every snap of every
  thread as full structs (media paths, hashes, durations, locations) —
  just to read each thread's last-snap media-type/direction and to seed
  the in-memory dedup set. For C conversations averaging S snaps that's
  O(C) queries hydrating O(C·S) full structs, on every reload (which fires
  on reconnect/restore). Replaced with a FIXED THREE queries regardless of
  conversation count: `loadConversations` (summaries, already existed),
  new `loadConversationLastMeta` (per-partner last-snap media-type +
  direction in one GROUP-BY using SQLite's documented bare-columns-with-
  MAX behavior), new `loadSnapStubs` (dedup set — id + sender + recipient
  + viewed ONLY, no media). Work drops to 3 queries loading O(C) summary
  rows + O(C·S) lightweight stub rows. Correctness preserved: the stub
  carries exactly the fields addSnap's dedup AND the expiry-bookkeeping
  path (conversationKey / inbound-ness at InboxWidget ~726) read — I
  caught mid-change that an id-only stub would have broken expiry (it
  reads senderId/viewed), so the stub carries those too. Viewer still
  re-hydrates full snaps from storage on open (never relied on m_snaps
  for media).
  FOLLOW-UP (noted, not done — lower impact, wider blast radius):
  `resolveDisplayName` still does one `loadContact` per conversation in
  the same loop (another N+1, but single-row indexed lookups, and it's
  called from several places). Batch via a one-shot loadContacts→QHash if
  it shows up in profiling.


- [x] **🟢 SHUTDOWN crash ROOT-CAUSED via backtrace (V32.34+) — it was NOT
  the QProcess teardown class.** A clean `coredumpctl gdb` → `where` on the
  M2 Ultra (SC_UNIFIED_NAV=1 run, 34.5s, NO camera, NO swipe) finally named
  the crashing frame, and it is Qt-internal, not ours:
    ```
    #5  malloc_consolidate                         (heap corruption detected)
    #8  QHashPrivate::Data<...QRenderRule>::~Data   (freeing a render-rule cache)
    #9  QHash<QObject*, QHash<int, QHash<...QRenderRule>>>::erase
    #10 QStyleSheetStyleCaches::objectDestroyed(QObject*)
    #11 doActivate → QObject::destroyed(QObject*)
    #13 ~QWidget → ~QLabel / ~QPushButton (a STYLED widget being destroyed)
    ...
    #46 SettingsPage::~SettingsPage  (its theme-carousel subtree)
    #50 SwipeNavigator::~SwipeNavigator (SwipeNavigator.cpp:115)
    #56 main (main.cpp:318 — `return rc`, the widget tree unwinding)
    ```
  This is the long-standing Qt `QStyleSheetStyleCaches` corruption
  (QTBUG-11658 / QTBUG-48213): the stylesheet machinery tracks each styled
  widget in a per-object QRenderRule cache via the widget's destroyed()
  signal; with MANY per-widget setStyleSheet calls (we have ~365) plus a
  global app stylesheet, Qt can end up with cache entries whose destroyed()
  connections outlive the QStyleSheetStyle that owns them. At shutdown,
  destroying a styled widget (here the SettingsPage theme-carousel subtree:
  HorizontalScrollArea / CarouselAwareScrollArea / styled QPushButtons +
  QLabels) writes into a stale cache → fastbin corruption, detected at free.
  NOTE: this CORRECTS the earlier theory below — the QProcess orphan fixes
  (still valid in their own right) were NOT this crash; this run had no gst
  process at all. The two are independent.

  FIX V32.34+ (main.cpp, after exec() returns, before the widget tree
  unwinds): `qApp->setStyleSheet(QString()); window.setStyleSheet(QString());
  QCoreApplication::processEvents();`. Clearing the global + main-window
  stylesheet while the QApplication and its style are STILL ALIVE makes Qt
  re-resolve/evict the render-rule caches in a controlled order now, so by
  the time `window` is destroyed there are no stale cache entries left to
  corrupt. This is the established, Qt-version-independent workaround; it's
  purely teardown-ordering, no in-session behavior change. NEEDS DEVICE
  CONFIRM — if it still aborts at shutdown, the next lever is reducing the
  per-widget setStyleSheet proliferation (move to the global stylesheet with
  object-name selectors) on the SettingsPage theme carousel specifically,
  since that's the subtree in the backtrace. CAVEAT for my own new code:
  UnifiedNavBar also uses per-widget setStyleSheet AND deleteLater()s styled
  QPushButtons at runtime (clearSegments) — the SAME aggravating pattern; it
  didn't cause this (crash predates it) but watch it doesn't increase the
  frequency. If it does, switch the bar's segment styling to objectName
  selectors in one stylesheet rather than per-button setStyleSheet.


  shutdown crashes: run 1 (`SC_SMART_RIBBON=1 SC_UNIFIED_NAV=1`) ended in
  `malloc_consolidate(): unaligned fastbin chunk` (heap corruption) at
  ~14.5s with the camera just started; run 2 (`SC_UNIFIED_NAV=1` only,
  which gates NOTHING — that flag isn't wired yet) viewed a video, the
  snap expired + closed, then `segmentation fault` on exit.

  ROOT PATTERN (found in FOUR places): a QProcess teardown that does
  `setParent(nullptr)` (orphan) + `deleteLater()`. deleteLater only
  deletes when the event loop pumps again; at app exit (teardown from a
  destructor) the loop may never spin, and the orphaned parentless
  QProcess is then torn down inconsistently by static teardown → heap
  corruption / segfault. The orphaning REMOVES the parent-dtor safety net
  that would otherwise clean it up. Fixed all four by gating on
  `QCoreApplication::closingDown()`: when closing down, reap (SIGKILL
  already sent) + `delete` DIRECTLY (signals already disconnected, so no
  slot can fire); during normal operation keep the orphan+deleteLater path
  (loop pumps; avoids synchronous delete re-entering a mid-swipe handler
  stack). Sites fixed:
    * `VideoSnapViewerDialog::teardownGst()` — matches run 2 exactly
      (the code's own comment documented this open→expire→close→exit
      crash from a prior version; the prior "fix" swapped one leak for
      this orphaning).
    * `GstLaunchBackend::stop()` — STRONGEST candidate for run 1 (camera
      was active; this is the camera's gst backend). Same orphan pattern.
    * `VideoRecorder` proc teardown — same pattern.
    * `VideoTranscoder` proc teardown — same pattern.
  (`WebSocketTransport`'s setParent(nullptr) is NOT this bug — it's a
  deliberate ownership transfer to a unique_ptr, which deletes
  deterministically without the event loop. Left as-is.)

  CAVEAT: heap corruption is non-deterministic, so I can't PROVE these
  fixes cover run 1 from the log alone — but GstLaunchBackend::stop() is
  the same bug class and the camera was the active subsystem, so it's the
  most likely cause. DEVICE DIAGNOSTIC if it still crashes: the core was
  dumped — `coredumpctl gdb` then `bt` for the exact frame. Re-test both:
  (a) open a video, let it expire, quit; (b) just open with camera, quit
  quickly — both should exit cleanly now.


- [x] ~~**snapclone-firewall: ufw detected as active when it's
  inactive.**~~ **Fixed V32.34+ (found auditing the second zero-test
  crate; verified with new tests).** `ufw_is_active()` in detect.rs
  classified `ufw status` output with `.contains("active")` — but
  "active" is a substring of "in**active**", so a disabled-but-installed
  ufw read as ACTIVE and was selected as the firewall backend. The bridge
  would then add allow-rules to a firewall that isn't enforcing anything
  (rules sit in an unused ruleset; the port's real reachability is decided
  elsewhere). Fixed to match the full lowercased "status: active" token
  and explicitly reject "inactive". Extracted a pure `parse_ufw_active()`
  so the classification is unit-testable without invoking ufw; added 10
  tests (active/inactive/empty/garbage classification, backend name
  stability, needs_privilege, default backend, protocols expansion, the
  Backend::None no-op, default rule, protocol as_str). Also removed an
  unused `warn` import (the crate now builds warning-clean). **Reach:**
  the C++ app uses its OWN `src/network/FirewallPortManager.cpp`, which
  detects ufw by binary-presence (`findExecutable("ufw")`) — a DIFFERENT
  approach that does NOT have this substring bug. So the live app is safe;
  this bug was introduced only in the Rust port (which isn't linked or
  depended on by anything yet) and is fixed there for when it's wired.
  Testable Rust crates: 60 tests green (audio 8, saliency 8, net-intel 9,
  proximity 8, mailbox 11, theme 6, firewall 10).

- [x] ~~**snapclone-theme: palette_by_id silently dropped 3 themes +
  tokens drifted from the C++ source of truth.**~~ **Fixed V32.34+
  (found by auditing a zero-test crate, verified with new tests).** Two
  issues in `rust/snapclone-theme`: (1) `palette_by_id()` in lib.rs had a
  `match` that only handled 6 of the 9 palettes — `earth`, `pastels`, and
  `metallic` (which exist as constants, are exported, and are listed by
  `PaletteId::all()` + mapped by `from_str`) fell through to the
  `_ => DARK` default, so selecting any of those three in settings would
  silently apply Dark. Fixed by routing `palette_by_id` through a new
  exhaustive `palette_for(PaletteId)` (no `_` wildcard, so the compiler
  now FORCES a new arm when a palette variant is added — structural guard
  against this drift recurring). (2) `tokens.rs` claimed "Matches Theme::
  constants in Theme.hpp" but held the OLD pre-tightening values
  (FONT_BASE 15 vs current 13, SPACE_XL 48 vs 43, TOUCH_MIN 44 vs 40,
  etc.) — Theme.hpp was tuned down for the FP5 screen over several
  versions and the Rust mirror was never updated. Re-synced to the
  authoritative C++ values and corrected the comment to name Theme.hpp as
  source of truth. Added 6 tests (every id resolves to its own constant,
  string lookup matches for all 9, the earth/pastels/metallic regression,
  as_str/from_str round-trip, unknown→Dark fallback, all() completeness).
  **Reach:** snapclone-theme is NOT consumed by the C++ app (which uses
  its own Theme.hpp) nor by other Rust crates — it's staged for a future
  cross-backend (Qt/egui/Slint/web) unification. So like the net-intel
  fix below, this corrects real latent bugs that would bite when the crate
  is wired, not live phone behavior today. Testable Rust crates: 50 tests
  green (audio 8, saliency 8, net-intel 9, proximity 8, mailbox 11,
  theme 6).

- [x] ~~**snapclone-net-intel path-scoring confidence was inert.**~~
  **Fixed V32.34+ (found via the Rust test suite, not logs — the crate's
  own tests were failing).** `PathScore::confidence()` in
  `rust/snapclone-net-intel/src/scoring.rs` weighted each side by a single
  decay factor from the MOST-RECENT timestamp (≤1.0) instead of by the
  NUMBER of observations. So five successes counted the same as one,
  `total` capped near 1.0, and the `evidence = total/5` term capped at
  ~0.2 — meaning confidence could never exceed ~0.6 (successes) or drop
  below ~0.4 (failures); the blend-toward-0.5 term dominated forever.
  Consequence: `is_warm()` (needs >0.7) NEVER fired and `is_negcached()`
  (needs <0.2) NEVER fired, so the path-preference ranking in `memory.rs`
  was flat and negative caching never engaged — the whole scoring system
  was decorative. Fix: multiply each decay by the success/failure count so
  evidence actually accumulates (N recent successes → weight≈N → evidence
  saturates → confidence approaches the raw ratio). The crate's 2 failing
  tests now pass; added 5 more (warm/negcached predicates firing,
  mostly-success-with-one-failure stays confident, single-observation
  stays mildly-positive) to lock the semantics. Reach: net-intel isn't
  linked into the C++ app yet, but `snapclone-session` depends on it, so
  the fix is live in that crate graph for when session scoring is wired.
  All currently-offline-testable Rust crates green (audio, saliency,
  net-intel, proximity, mailbox = 44 tests).

*Caught by reading session logs rather than hitting them in code review.
Each item should have evidence in a saved log — file the log alongside
when filing the bug. These are real-app-runtime issues that mature
logging frameworks would normally surface via dedicated helpers (panic
hooks, paint-call validators, gesture-conflict detectors). Until that
infra exists, this is the catch list.*

### Active runtime issues

- [x] ~~**PQ handshake (SHK2) duplicate storm → MAC verification
  failures.**~~ **Fixed V32.34+.** Field logs showed repeated
  `SHK2 responder: no pending ephemeral — dropped` and
  `Decrypt failed: MAC verification failed` for the same peer right
  after connect. Two bugs, both in the v2 SHK2 path in
  `AppController_connect.cpp`:
  (1) **Dead guard.** The `responder=false` branch (peer initiating PQ
  with us) had a `handshakeRespondedTo` guard that INSERTED into the set
  but called `sendShakV2(asResponder=true)` UNCONDITIONALLY outside the
  `if` — so the guard never gated anything. Duplicate SHK2 blobs (the
  same handshake redelivered across UDP punch + relay + gossip — these
  are control messages, so they bypass the snap-nonce dedup) each re-ran
  `respondHybridHandshake`, which ENCAPSULATES FRESH every call → a new
  ct + new session key each time, each overwriting `d->pqSessionKeys`.
  The peer finished from the FIRST ct, so once our stored key rotated to
  a later duplicate's key, every decrypt (v1 AND v2, since they share
  the registered session key) hit MAC-verification failure. Fix: gate
  the resend so the first response wins and the session key stays
  stable; duplicates return early.
  (2) **Misleading warning.** The `responder=true` branch warned
  "no pending ephemeral — dropped" on duplicates of the final ct (the
  first copy completed + erased the ephemeral). Now distinguishes a
  completed-handshake duplicate (debug log, expected) from a genuine
  lost-ephemeral case (warning) by checking whether a session key
  already exists.
  Also fixed: the `peerDisconnected` cleanup cleared the guard only
  under the transport id, but the v2 guard keys on the normalized stable
  id (which can differ) — now clears both so a genuine reconnect always
  re-handshakes. (Network-layer dedup of control messages is a separate,
  larger item; the per-peer state guards are the correct handshake-layer
  fix.)
  **V32.34+ follow-up (initiator side):** found a THIRD related bug while
  re-auditing. The `sendShak()` retry loop fires every 2s (up to 4
  attempts) until the v1 ack arrives, and each retry called
  `sendShakV2(asResponder=false)` which generated a FRESH ephemeral
  ML-KEM keypair and OVERWROTE `pendingPqEphemerals[peer]`. If a retry
  fired while the peer's ct (encapsulated against the previous ek) was in
  flight, the responder path would decapsulate that ct with the NEW
  ephemeral's dk → wrong/garbage session key (or finishHybridHandshake
  fails), and the later ct found no pending ephemeral. Fix
  (`AppController_misc.cpp`): the initiator branch now REUSES an existing
  pending ephemeral (re-ships the stored ek) instead of regenerating, so
  the dk stays stable across retries within a handshake window and
  whichever ct arrives first completes correctly. Forward secrecy
  preserved: `pendingPqEphemerals` is still cleared on disconnect so a
  genuine reconnect generates a fresh ephemeral. Considered also stopping
  the retry on `pqSessionKeys` presence but REVERTED that — pqSessionKeys
  deliberately persists across reconnects ("derive once, reuse"), so a
  stale entry would wrongly suppress the reconnect handshake;
  handshakeRespondedTo (cleared on disconnect) remains the correct retry
  gate.

- [x] ~~**Video snaps stop playing ~1s after open / segfault on
  exit.**~~ **Fixed V32.34+.** Regression surfaced by the V32.34
  `closeViewerForSnap` change. Two coupled bugs: (1) `markSnapViewed`
  started the view-expiry timer from `durationSecs` (the Snapchat-style
  "how long you may look" window), which for these snaps clamped to the
  1s minimum — so the snap expired ~1s after opening and the new
  expiry→close wiring tore down the viewer mid-playback (before, the
  early expiry was silently ignored so nobody noticed). Fix: for video
  snaps, the expiry is now `max(durationSecs, ceil(videoDurationMs/1000)
  + 2s grace)` so the clip always plays to EOS before close. (2) The
  rapid open→expire→close cycle exposed a teardown bug: `teardownGst`
  used `kill()` + reparent + `deleteLater` with NO `waitForFinished`,
  relying on the event loop to pump again to delete the QProcess — but
  at shutdown the loop may not spin, so the orphaned QProcess was
  destroyed while gst-launch was still running (`QProcess: Destroyed
  while process is still running`) → segfault on exit. Fix: after
  SIGKILL, `teardownGst` now does a bounded `waitForFinished(200)` to
  reap the child deterministically before deleting the wrapper (safe
  because all signals from the QProcess are disconnected first, so the
  brief local event loop can't re-enter our slots).

- [x] ~~**Connection-request mailbox builds malformed `ws://ws://`
  address.**~~ **Fixed V32.34+.** Field log showed `connectToPeer:
  refusing invalid target "ws://ws://69.156.66.68:9876"` — a wasted
  connection attempt (peer still reached via relay fallback, so it
  wasn't fatal, just slow + log-noisy). Root cause: the connreq
  producer in `AppController_commands.cpp` wrote the address via
  `PeerAddress::toString()` (which adds `ws://`), but the receiver's
  `parseAddr` in `AppController.cpp` treats `PeerAddress.host` as
  schemeless and `toString()` re-prefixes → double scheme. The OTHER
  connreq producer (`AppController_connect.cpp:856`) already wrote bare
  `host:port`, so the two producers were inconsistent. Fixed both ends:
  receiver `parseAddr` is now scheme-tolerant (strips ws://|wss://,
  sets tls, handles a doubled prefix defensively, strips trailing
  path), and the commands.cpp producer now writes bare `host:port` to
  match the connect.cpp producer. Wire format is now uniform.

- [x] ~~**Connreq blob storm — same blob processed 4-6× per DHT GET.**~~
  **Fixed V32.34+.** Field logs showed `Connection request blob from
  "sc-XXXX" (117 bytes) — passing to AppController` logged at INFO level
  4-6 times in a row per mailbox check, followed by `[MAILBOX]
  Duplicate nonce — skipped N copies`. Cause: OpenDHT replicates each
  stored value across multiple nodes, so one GET returns the same blob
  several times; every copy was logged AND posted to AppController via a
  queued cross-thread invokeMethod, where the nonce-dedup finally
  dropped it — after all the redundant work. Fix: added a per-GET-batch
  `seen` set (keyed on the full raw blob bytes) in the connreq GetCtx in
  `JamiAdapter_dht_impl.cpp`, collapsing identical replicas to a single
  log + forward at the source. The AppController-side nonce dedup (with
  its aggregated flush-timer logging) stays as the complementary
  cross-batch guard for the near-simultaneous periodic/startup/queue-
  exhausted checks that fire in the same second.

- [x] ~~**Ephemeral snap dialog relies on the stuck-dialog safety timer
  to close.**~~ **Fixed V32.34+.** When `snapViewExpired` fires (the
  timed view window elapsed), MainWindow now calls
  `InboxWidget::closeViewerForSnap(id)`, which looks the snap up in the
  `m_openViewers` map and calls `close()` on the open dialog. With
  `WA_DeleteOnClose` this runs the dialog's destructor → `teardownGst()`,
  tearing down the gst-launch playback process cleanly. Closes both
  video (`VideoSnapViewerDialog`) and image (`SnapViewerDialog`) viewers
  uniformly. Result: timed snaps now close exactly when their duration
  elapses (enforcing ephemerality) instead of lingering ~4s until the
  MobileDialog safety timer auto-rejected them. The `destroyed()`
  handler remains the single owner of the `m_openViewers` entry removal,
  so there's no double-remove.

- [ ] **Video playback intermittent — one of two videos plays, second
  one stuck.** Phone A log 2026-05-15: first .webm played 4s and
  exited clean (`gst-launch exited code 0`), second .webm stuck
  until the dialog stuck-dialog safety timer fired at 15s and
  killed gst-launch via QProcess destroy. Same pipeline both times.
  Suspect: audio sink contention OR dialog lifecycle race. Need
  reproducer with `GST_DEBUG=3 gst-launch-1.0 ...` to capture pipeline
  state at hang.

- [ ] **Two-phone divergence: camera green-tint AND video playback
  on different pmOS edge channels.** Two Fairphone 5s running
  different `postmarketos-edge` update levels showed materially
  different behavior:
    - One displayed the camera viewfinder correctly; the other had
      the heavily green-tinted stride-mismatch mess.
    - One could play back received .webm videos; the other failed
      pipeline construction on the same files.

  **Hypothesis (camera side, partly mitigated V32.34+):** GStreamer
  caps negotiation lands on different default formats depending on
  the version of `pipewire`, `gst-plugin-pipewire`, and `libcamera`
  installed. The newer channel may default to 3-byte RGB through
  the pipewire path; older defaults to 4-byte BGRx. Our pipeline
  used to pass either through to the QImage parser. V32.34+ added
  an explicit `format={BGRx,BGRA,RGBA,RGBx}` caps filter after the
  first videoconvert, forcing negotiation into a 4-byte channel
  before the BGRx coercion. This should resolve the camera green
  tint regardless of channel, but until both phones confirm,
  consider it patched-not-validated.

  **Hypothesis (video playback side, NOT YET FIXED):**
  `VideoSnapViewerDialog`'s pipeline is `filesrc ! decodebin name=dec
  dec. ! queue ! videoconvert ! videoscale ! video/x-raw,pixel-aspect-ratio=1/1
  ! jpegenc ! multifilesink`. decodebin autoplugs codec decoders
  internally based on file MIME — and the autoplug decisions diverge
  between gstreamer versions / installed plugin sets. On channels
  where the chosen decoder produces a format `videoconvert` doesn't
  handle cleanly (e.g. AV1 emitting Y444_10LE on some plugins),
  negotiation fails silently with "not-negotiated (-4)".

  **What to collect when reproducing:**
   1. On BOTH phones: `gst-inspect-1.0 --version` to compare GStreamer.
   2. `apk list -I | grep gst-` (or pmOS equivalent) to compare
      installed plugin sets.
   3. `pkg version pipewire libcamera` for the system-side rendering
      stack version.
   4. With the V32.34+ Settings → Debug & FSM Tuning → "Mobile
      display debug" toggle ON, the user reproduces the failure
      and grabs the log. The camera/video log categories
      (`snapclone.camera.gst`, `snapclone.video.viewer`) emit full
      stderr from gst-launch — that's where caps negotiation
      failures and decoder autoplug errors appear.
   5. Run the failing pipeline manually with `GST_DEBUG=3 gst-launch-1.0
      ...` using the exact args our backend logs.

  **What to consider for the video playback fix:**
    - Add an explicit `videoconvert` immediately before videoscale
      (parallel to the camera-side fix) — costs a no-op when
      negotiation already worked.
    - Constrain decodebin's output via a sink caps filter:
      `decodebin ! video/x-raw,format={I420,NV12,YUY2,BGRx} ! ...`
      forces 8-bit standard-gamut formats and rejects exotic ones
      that downstream can't handle.
    - Probe the file FIRST (gst-discoverer-style) to know what codec
      decodebin will choose, then build a pipeline tailored to that
      codec. More complex but bulletproof against autoplug
      divergence.

  **Channel sensitivity is a real and ongoing risk** — pmOS edge
  channels move independently and a fix that works on Channel A
  one week can break on Channel B the next when an unrelated
  gstreamer plugin updates. Build resilience into our pipelines
  (explicit caps, format constraints, fallback decoders) rather
  than trying to chase the upstream changes.

- [~] **Received video snap displayed wrong orientation.** V32.34+:
  root-caused as a DOUBLE-rotation. The recorder bakes the chosen
  rotation into the pixels via its own `videoflip` before encoding, so
  the stored frames are upright. But the container (matroska) and/or
  H.264 SEI can ALSO carry a rotation tag that some players honor,
  rotating a second time on the receiving end — which is why the
  sender's own viewfinder looked fine (never round-trips the tag) but
  the recipient saw it sideways. **Fix applied:** the playback pipeline
  in VideoSnapViewerDialog now inserts `videoflip
  video-direction=identity`, forcing playback to trust the pixels and
  ignore any stream rotation tag (a no-op for untagged files).
  **Still to verify on device:** confirm this resolves it for all four
  capture rotations, and check the inbox THUMBNAIL path (separate from
  the viewer) for the same double-rotation. Also still open: capture
  rotation is a fixed user setting (`camera/userRotation`, rotated via
  a button), NOT auto-sensed from device orientation — so a sender who
  hasn't set their rotation correctly bakes a wrong orientation that no
  receiver-side fix can recover. Real fix needs device-orientation
  sensing at capture (QScreen::orientation / a sensor), filed below.

- [ ] **Camera white balance — color cast on the pipeline.** Reported
  V32.34+. Analysis: white balance on the FP5/pmOS path is libcamera's
  job (its AWB algorithm runs in the ISP/IPA, upstream of pipewiresrc),
  and `pipewiresrc` exposes no AWB/color-temperature control we can set
  from GStreamer. GStreamer has no stock AWB element either —
  `videobalance` only does brightness/contrast/hue/saturation, NOT
  per-channel R/G/B gains, so it can't neutralize a color cast.
  Realistic options, none free:
    1. **libcamera IPA tuning** — the correct fix, but it's device-
       firmware-level (a tuning file for the sensor), outside this app.
       Worth checking whether pmOS ships a tuned file for the FP5
       sensors or falls back to a generic one (generic = bad AWB).
    2. **Gray-world AWB in our frame path** — compute per-channel mean
       and scale R/B gains to equalize them, applied to the BGRx QImage
       in GstLaunchBackend. CHEAP (~1-2ms/frame on A55) BUT only fixes
       the live PREVIEW + stills — the recorded video that recipients
       see goes through the SEPARATE recorder pipeline straight to the
       encoder, never touching that QImage. So this would make the
       preview look right while recipients still get the cast — a
       confusing half-fix. Would need the same correction inserted into
       the recorder's encode branch too (a custom GStreamer element or
       an appsink→gain→appsrc loop, which is heavy).
    3. **A LUT/`coloreffects`-style correction** baked into both the
       preview and record pipelines — but a static LUT can't adapt to
       lighting, so it'd help one scene and hurt another.
  Recommendation: first confirm whether it's a libcamera-tuning issue
  (check `cam --list` / libcamera logs for which tuning file loads). If
  pmOS uses a generic tuning, option 1 upstream is the real fix. Only
  if that's a dead end is the gray-world-in-both-pipelines work (option
  2 extended) worth the per-frame cost on the recorder side.

- [ ] **App crash on save.** Reported by user; no log signature
  captured. Diagnostic commands filed (see "Logging gotchas" below).
  Next reproduction: grab `dmesg -T | grep snapclone` and
  `coredumpctl list` immediately after.

- [ ] **Ambient layer cannot be disabled.** No in/out toggle. Two
  derivations within 2 seconds despite the 30s throttle (Phone B
  log 2026-05-15 13:51:16, 13:51:18). Throttle may be per-instance
  and instance gets recreated, or throttle code has a bug. Ambient
  also starts/stops on every page change rather than persisting.
  Blocked on: build a Settings toggle FIRST so perf can be measured
  with ambient on vs off. Without the toggle, ambient drowns other
  perf signal.

- [ ] **Swipe gesture handler — architectural rewrite in progress.**
  Phase 1 (top-down dispatch) shipped V32.33. Replaces hybrid
  TouchBegin/TouchUpdate-mid-stroke-reclassification with single
  `classifyIntent()` call at TouchBegin that owns the gesture for its
  lifetime. Resolves the original "swipe handler too greedy" symptom
  by ensuring the gesture commits to one intent (PassToChild,
  ChildScrollV, ChildScrollH, PageSwipe) and dispatches without
  reclassification.

  **Known limitation of Phase 1:** horizontal flick inside a
  vertical-scrolling area is now silently swallowed (no page-swipe
  fall-through). This is architecturally correct per the no-mid-stroke-
  reclassification design, but it IS a UX change worth watching for.

  **Phase 2+ planned:** see `docs/GESTURE_AFFORDANCE_MIGRATION.md`
  for the full migration plan. Adds an Ambiguous gesture state that
  shows affordance hints (consider-state on widgets, scrollbar
  pre-show, page-edge peek) before committing to a concrete intent.
  Multi-turn, app-wide. Captures the desired behaviors that should
  survive the migration so they aren't lost during rollout.

- [x] ~~**QPainter unbalanced save/restore on page swipe**~~ **Fixed
  V32.34+.** The outgoing page got a `QGraphicsOpacityEffect`
  (1.0→0.65 fade) applied DURING the slide — i.e. an effect attached to
  a widget being simultaneously geometry-animated and repainted. Qt then
  ran the effect's pixmap-grab render path while the slide paint was
  still active on the same device → the storm of `QPainter::begin: A
  paint device can only be painted by one painter`,
  `QWidgetEffectSourcePrivate::pixmap: Painter not active`, and
  `Unbalanced save/restore` warnings on every swipe (worst on Contacts,
  whose cards also carry per-card opacity effects, compounding it). Fix:
  removed the page-level fade entirely — the geometry slide IS the
  transition; the subtle fade wasn't worth the painter contention. Also
  added a defensive `setGraphicsEffect(nullptr)` on the outgoing page at
  slide start to clear any stale effect. If a cross-fade is wanted back,
  the correct approach is rendering each page to a static QPixmap once
  and cross-fading the pixmaps — never attaching an effect to a moving
  live widget.

- [ ] **Rear camera shows front camera (Phone B).** `pw-dump found
  QList()` repeatedly, then optimistic baseline starts
  `virtual-cam-rear`. Either the camera bridge isn't creating
  virtual cameras, or PipeWire can't enumerate them. Diagnostic
  commands filed below. PIP feature blocked on this — needs
  `availableDevices()` to return both rear and front.

- [ ] **Duplicate snap reception — path-convergence needed.** Same
  snap arrives 3-4 times across UDP punch + relay + gossip paths.
  Dedup correctly suppresses UI emission, but the sender keeps
  fanning out across all transports even after the network has
  proven a working path.

  **Approach (per user discussion):** aggressive then back off,
  framed as path-convergence with failure-detection escalation:

  - **Phase 1 — Unknown.** First send to a peer: fan out across
    all available transports (current behavior). Accept redundancy
    because we don't know which path will succeed.
  - **Phase 2 — Converged.** When ACK arrives via path P, mark P
    as preferred. Subsequent snaps go via P only. Other paths drop
    to a minimal keepalive cadence (enough to detect P failing).
  - **Phase 3 — Recovery.** If P stops ACKing within a tight window
    (~3-5s on a sent snap), re-fan-out to all paths and return to
    Phase 1.

  **State per peer:** `preferred_path`, `path_last_ack_at`,
  `convergence_state ∈ {Unknown, Converged, Recovering}`. State
  machine transitions on every ACK + on every send + on path
  timeout.

  **Risks to handle:**
  - Failure detection window choice has UX consequences: too tight
    → fan-out fires on transient packet loss; too loose → snaps
    sit unsent when the converged path is silently dead.
  - "Snap never arrived" is invisible from logs but very visible
    in UX, vs current "snap arrived 4 times" which is loud but
    correct from the user's POV. The fix must not make the
    invisible failure more common.
  - Mobile networks drop paths unpredictably. Phase 3 detection
    needs to be quick and the keepalive on non-preferred paths
    needs to be cheap-but-real.

  Lives in `SnapRouter` send-path logic; receive-side
  `m_recentInbound` dedup stays as-is (silently absorbing
  redundancy is the correct receive-side behavior — defense in
  depth).

  **V32.34+ partial (receiver-side dedup-consistency).** While the
  sender-side convergence above is still open, found + fixed a related
  receiver-side bug: the UI's two snap-ingestion paths didn't share
  dedup state. `InboxWidget::addSnap()` dedups via `m_snaps` (keyed by
  snap id), but `reloadConversationsFromStorage()` rebuilt
  `m_conversations` + `m_unreadCount` from storage WITHOUT populating
  `m_snaps`. So after a reload, a live arrival of a snap storage had
  already counted missed the `alreadyKnown` check, re-ran
  `updateConversationFromSnap`, and double-counted the unread badge +
  inflated the conversation aggregate — a duplicate that SnapRouter's
  `m_recentInbound` correctly absorbed at the network layer could still
  double-count at the UI layer if a reload sat between the two
  receptions. Fix (`InboxWidget.cpp`): reload now clears + repopulates
  `m_snaps` from the same `loadConversationSnaps` threads it already
  loads for the row preview (no extra queries), so a later live
  duplicate is recognized as known. Also added the missing dedup guard
  to the legacy flat-list path (it appended a row per call with no
  alreadyKnown check). Safe because SnapRouter persists snaps to storage
  BEFORE emitting inboundReady, so any snap the UI has seen is already in
  storage and survives the reload's m_snaps rebuild.

- [x] ~~**SYNC dedup window.**~~ **Done V32.34+.**
  `SnapRouter::handleV3Sync` now checks per-peer `m_lastSyncHandledMs`;
  SYNC requests arriving within 250ms of a previous-handled one for
  the same peer are dropped at debug level (silent at default log
  config). Eliminates the "peer knows 0 hashes" rapid-fire pattern
  observed on Phone A.

- [x] ~~**Mailbox dedup log spam.**~~ **Done V32.34+.** Replaced the
  per-occurrence `qCDebug` with a 500ms flush-timer-coalesced summary.
  Same nonce dup counts increment; after 500ms of quiet, one summary
  line fires: `[MAILBOX] Duplicate nonce from <peer> — skipped N
  copies`. Reads as ONE line per mailbox check regardless of how many
  neighboring DHT keys yielded the same blob.

- [x] ~~**Memory leak audit pass.**~~ **Done V32.34+.** Full sweep of
  long-lived collections in core, network, ui, crypto. Fixed:
    - `Impl::peerPunchAddr` — was unbounded, now bounded by both
      a 1024-entry hard cap (with batch eviction) AND a 30-min TTL
      sweep every 5 minutes
    - Per-peer state on disconnect (`mailboxPublished`,
      `relookupCount`, `pendingDhtLookups`, `activeLookupHandles`,
      `lookupGeneration`, `pendingPqEphemerals`) — was retained
      forever per-ever-contacted-peer; now removed on `peerDisconnected`
    - `SnapRouter::m_lastSyncHandledMs` — new in V32.34+, added
      to `clearPeerState` (called on disconnect)
    - `SwipeNavigator::m_filteredWidgets` — was holding dangling
      pointers; now connects `QObject::destroyed` for auto-removal
  Audit doc with full method + outstanding (filed) items lives at
  `docs/MEMORY_LEAK_AUDIT.md`.

- [ ] **Bounded cap for v1+v2 session keys (filed by memory audit).**
  `CryptoService::m_sessionKeys` and `RustCryptoService::m_sessionKeys`
  and `Impl::pqSessionKeys` all retain keys forever per ever-met peer.
  Design intent is reuse-across-reconnects (perf vs forward-secrecy
  trade-off taken at V32.27 PQ rollout), so we don't want to clear
  on disconnect. But unbounded growth needs a backstop.

  **Design sketch:** LRU eviction at 256 peers. Requires:
    - Add `forgetPeer(peerId)` to ICryptoService interface
    - Track per-key access timestamp in a parallel map
    - On insert, if size >= cap, evict the least-recently-accessed entry
    - Same pattern for `pqSessionKeys` in AppController_p

  ~half-day work. Not urgent: most users have <100 lifetime peers.

- [ ] **NetworkStats TTL eviction (filed by memory audit).** Per-peer
  transport-class profiles in the `NetworkStats` singleton accumulate
  forever (~200 bytes per peer). Add a periodic sweep that drops
  entries with `lastAttemptMs` older than 7 days. The ranking
  algorithm gives near-zero recency-score to old entries anyway, so
  dropping them costs nothing functionally.

- [ ] **NetworkManager m_lanFailureCount TTL (filed by memory audit).**
  Per-peer LAN-attempt-failure counter. Cleared on successful LAN
  connect but never on TTL. A peer reachable only via relay
  accumulates an entry forever. Pattern: extend `pruneDiscoveryMap()`
  to also prune lanFailureCount on the same 60s cadence.

- [x] ~~**Defensive camera quality management.**~~ **Done V32.34+.**
  User-reported: "cameras when they work get stuck in low quality
  modes." Root cause: libcamera/pipewire negotiate the LOWEST
  resolution the caps allow. Without explicit source-side caps the
  pipeline lands on the sensor's smallest preview mode (often
  320×240). Four-part fix:

  1. **CameraPolicy gains sourceTarget{Width,Height,Fps} fields.**
     Default 1280×720@30. Settings can override per-install. Set
     to 0 to disable the constraint (legacy "let pipewiresrc pick").

  2. **GstLaunchBackend asks pipewiresrc for those caps.**
     Inserts `! video/x-raw,width=W,height=H,framerate=F/1 !` right
     after `pipewiresrc`. Drives libcamera to pick the matching
     sensor mode.

  3. **Camera-bridge does the same upstream of its videoconvert.**
     Honors `CAPTURE_WIDTH`, `CAPTURE_HEIGHT`, `CAPTURE_FPS` env
     vars (defaults 1280×720@30). For the app's gst-launch consumers
     which read FROM the bridge's virtual-cam-*, this is what
     determines the actual sensor mode libcamera negotiates.

  4. **Parse negotiated caps from gst-launch stderr.** Added `-v`
     flag (gated by logFirstFrameInfo policy). Regex-extracts width
     and height from the pipewiresrc src-pad's `caps =` log line.
     Compares to target. If we got materially less (ratio <
     lowQualityRatio, default 0.75), logs a warning AND optionally
     restarts the pipeline once (`autoQualityRestart=first`, the
     default) — some libcamera drivers latch into preview mode and
     a restart-with-fresh-caps clears it.

  5. **Frame-rate health check.** Every 30 frames compute median
     inter-frame interval. If above target × frameRateDegradedRatio
     (default 1.5×), log ONCE per session that the framerate is
     degraded. Surfaces sensor stuck in low-fps mode OR CPU
     overload without making the user manually check.

  Settings UI exposure deferred (see follow-up below). Defaults
  are sensible for FP5 / modern phone sensors; users on weird
  hardware can edit ~/.config/SnapCloneProject/SnapClone.conf
  directly for now.

- [ ] **Settings UI for camera quality policy fields.** The new
  `sourceTarget{Width,Height,Fps}`, `lowQualityRatio`,
  `autoQualityRestart`, `frameRateDegradedRatio` fields are
  settable via QSettings but not yet exposed in
  `SettingsPage::buildCameraSection`. Pattern: spinboxes for the
  numeric fields, combobox ("off"/"first"/"always") for the
  restart mode. Should live in a "Quality" sub-section below the
  existing camera device selection. ~half-day work.

- [ ] **Camera-bridge: auto-recover from stuck preview mode.**
  The bridge's `FrameCapture` is built once at session start and
  doesn't renegotiate. If libcamera latches into a low-res mode
  AT THE BRIDGE LEVEL, the app's source-target caps can't fix
  it — the bridge would have to tear down its libcamera session
  and reopen with explicit hints. Detect via the same "actual
  size much less than CAPTURE_WIDTH/HEIGHT" check in `Frame`
  receive, and if so destroy + recreate the `FrameCapture` once.
  Filed because: (a) the app-side fix probably resolves the
  common case, (b) the bridge-side restart is more invasive and
  needs careful testing to avoid restart loops.

- [ ] **CameraPolicy: track per-DEVICE settings, not global.**
  Currently sourceTargetWidth applies to every camera. But the
  front camera on FP5 maxes at 480×640, while the rear does
  1080p. A global "request 1280×720" works for rear but the
  front silently downgrades (with our auto-restart in "first"
  mode, this also wastes one restart per session). Better: a
  per-device key map keyed by device ID, with global fallback.
  Requires saving the device's MAXIMUM resolution at registry-
  scan time (CameraRegistry already has a hook for this).

- [ ] **Two-phone PIP divergence (V32.34+ device test).** User
  testing on two FP5s revealed PIP fails differently per device:
   - **Phone A**: PIP small overlay doesn't show (rear cam itself
     renders green). Likely a CameraRegistry/discovery issue —
     `firstFrontDeviceId()` may return empty if the front node
     isn't classified correctly, hiding the PIP overlay.
   - **Phone B**: PIP fails entirely AND the fallback rear-cam
     view shows the FRONT sensor. This is a node-misnaming
     issue: bridge's `virtual-cam-rear` is wired to the FRONT
     hardware on this device.

  Bridge has autodetect via `cam --list` already (capture.rs:
  `parse_cam_list`, `pick_via_cam_list_multi`) but only fires when
  cam-list output is available. Hardcoded
  `DEFAULT_PW_NODE_REAR/FRONT = cci_ac4a000 / cci_ac4b000` is
  Phone-A-specific; on Phone B the i2c bus enumeration differs.

  **Diagnostic now logged at bridge startup (V32.34+):** full
  `cam --list` output AND `pw-dump` probed node list. Run after
  rebuild:
  ```
  journalctl --user -t snapclone-bridge | grep -A3 'cam --list\|pw-dump probed'
  ```
  Share the section between those markers — that pinpoints whether
  Phone B has wrong defaults, wrong cam-list, or wrong matching.

- [ ] **Green/purple color shift on incoming snaps (V32.34+).**
  User reports videos sent in green-and-purple arrive normal on
  laptop. So the SENDER's bytes are correct (laptop renders right);
  the PHONE'S rendering of those bytes is wrong. Pattern matches
  R/B byte swap: `Format_RGB32` (little-endian: [B,G,R,X]) bytes
  reinterpreted as `Format_RGBA8888` (defined byte order: [R,G,B,A])
  → R↔B swap, alpha=full. Red things appear blue, blue appear red,
  red+blue (purple) stay purple, green stays green.

  Candidate paths to audit:
   - SnapViewerDialog: how does the incoming JPEG get loaded?
     If `QImage::loadFromData` produces Format_RGB32 but a later
     conversion expects RGBA8888, we'd see this.
   - InboxWidget thumbnail generation
   - The pixmap-cache layer (if any)

  **Action item:** add a one-line log at the point a received
  image is loaded showing the QImage::format() integer. Compare
  what Phone A vs Phone B reports for the same .webp/.jpg payload.

- [x] ~~**Wayfinder + global UI sizes — additional 5% reduction.**~~
  **Done V32.34+.** Total 10% reduction from original V32.33 sizes
  (two iterations of 5%-down). All `Theme::TOUCH_*`, `FONT_*`,
  `SPACE_*` constants dropped. Wayfinder `kAvatarMax` 76→72 to
  match. TOUCH_MIN at 40 is at the comfortable lower bound for
  fingertip taps; further reductions need user testing to verify
  reliability.

- [ ] **Inbox redesign: sliding bottom tray of contacts.**
  User feedback: instead of the current list-by-conversation
  layout, switch to a tray that slides up from the bottom showing
  contacts sorted by recency. Optional center-distributed layout
  alternative. Tied to a filter button (first slot, left side).

  **Design sketch:**
   - Main inbox view is the current snap thumbnails / conversation
     summary, unchanged for the snap-content part.
   - Bottom: replace per-conversation list with a horizontal
     sliding tray of contact avatars. Avatar size like Wayfinder's
     "nearby" cluster (~50px circles). Tap avatar = open
     conversation. Long-press = compose new snap to that contact.
   - Filter button at left edge of tray: pops up a small menu
     for "All", "Online now", "Has unread", "By distance" (uses
     Wayfinder's distance data). Defaults to "By recency".
   - Center-distributed alternative: instead of left-to-right
     sliding, show the most-recent contact in the center, with
     older fading toward the edges. Same gesture (swipe horizontally
     to scroll) but visually different. User can pick in Settings.

  Risks: requires touching InboxWidget significantly. Existing
  filter / sort code can be reused. The "long-press to compose"
  behavior would need TouchFeedback / press-gesture wiring.
  Half-day to one-day work.

- [ ] **Magic Wormhole relay drops at ~30s.** Phone A log 13:48:26:
  `TCP error: The remote host closed the connection` after about
  40 seconds of relay use. Reconnect attempt starts but may or may
  not succeed. Pre-existing infra reliability of
  transit.magic-wormhole.io — not fixable client-side but worth
  having a reconnect-backoff strategy that doesn't hammer.

### Logging gotchas (helpers a mature app would have)

These are categories where the current logging is bad enough that
diagnostics are hard. Each one a "helper that doesn't yet exist."

- [ ] **No panic hook / crash logger.** When the app crashes, the
  log file just ends. No "Aborted" line, no signal info, no
  backtrace. A startup-installed signal handler that catches
  SIGSEGV/SIGABRT and writes a marker line + best-effort backtrace
  before re-raising would surface crashes that today are silent.
  See Rust's `std::panic::set_hook` and C++'s `std::set_terminate`
  + signal(SIGSEGV, handler). Minimal viable version: just log
  "SIGSEGV at PC=0xXXXX" and re-raise so coredump still happens.

- [ ] **No QPainter validator.** Qt's painter errors are emitted to
  qDebug but don't include the widget that caused them. Wrapping
  paint events with a save/restore depth tracker and logging
  "widget X opened a painter that wasn't closed" would point right
  at the culprit. Otherwise debugging is "grep for QPainter in
  every widget that paints."

- [ ] **No gesture-conflict surfacer.** When multiple gesture
  handlers compete (swipe vs scroll vs tap vs input focus), the
  current logs show all of them firing in parallel. A higher-level
  GestureArbitrator that logs "claim conflict: SwipeNavigator
  claimed gesture-id=X, but QLineEdit at pos was also a candidate"
  would make the eager-swipe issue diagnosable in one log line
  rather than 20.

- [ ] **No rate-limited log warnings.** Some warnings (camera bridge
  failure to parse pipeline, duplicate mailbox nonce, gst-launch
  pipeline negotiation failure) fire every N seconds. Each line
  is `[W]` severity but the noise drowns real signal. A `qCWarningRL`
  macro that rate-limits per-source-location to "first 5 then
  every 60s" with `(repeated 47 times)` suffixes would help.

- [ ] **No "what's running" startup banner.** Log shows individual
  subsystems coming up but no consolidated "build hash, version,
  rust crypto: on/off, models found: yes/no, network: ipv4 only"
  banner. Hard to tell at a glance whether a given log is from
  the build you think it is.

### Platform reality notes (empirically tested)

These are not bugs in SnapClone; they're things about the platforms
SnapClone runs on that affect feature feasibility. Captured so future
implementation work doesn't repeat the investigation.

- **FP5 / pmOS WiFi peer-to-peer modes (tested May 2026):**
  Supported interface modes via `iw list`: `managed`, `AP` only.
  WiFi Direct (P2P) code is compiled into wpa_supplicant 2.11 but
  the ath11k driver does not expose P2P to userspace: D-Bus
  introspection returns "P2P is not available for this interface."
  IBSS (ad-hoc) and 802.11s mesh: not in the supported modes list.
  NAN (WiFi Aware): chip declares capability via `iw phy` but
  userspace usability is unverified.
  **Implication:** Direct Mode must use AP+client architecture, not
  P2P or ad-hoc. This is the same architectural conclusion that
  applies regardless of P2P support since AP+client is more
  portable, so the constraint isn't blocking — it just removes one
  potential optimization.

- **FP5 / pmOS modem SIM-less capability (tested May 2026):**
  Without SIM, with default ModemManager policy: modem held in
  shutdown state (`failed: sim-missing`), no operations available.
  Without SIM, with manual QMI override (`qmicli --device=qrtr://0
  --dms-set-operating-mode=online`): network scanning works,
  signal info readable for cells the modem can camp on (typically
  3G/UMTS; LTE/5G measurement requires registration), serving cell
  identification works. AT commands still blocked by ModemManager
  "debug mode required" policy regardless of SIM state.
  **Implication:** SIM-less tower-based location IS feasible on this
  hardware with daemon override. Earlier scoping conclusion ("SIM
  required for modem features") was wrong; corrected to "SIM
  optional with appropriate setup, but power-cost ~50-200 mA when
  modem is forced online is the real constraint."

- **libqmi version requirement:** Direct QMI access requires libqmi
  1.30+ for QRTR socket transport. Alpine ships 1.38.0 which works.
  Older systems may not have QRTR support; would need libqmi upgrade
  or a different access path.

- **NetworkManager + wpa_supplicant control socket:** when NM is
  managing the WiFi radio, the legacy wpa_supplicant text control
  socket is not accessible (`wpa_cli` returns "No such file or
  directory"). To talk to wpa_supplicant: either use NetworkManager's
  D-Bus API, talk to wpa_supplicant D-Bus directly, or stop NM
  temporarily. The negative `wpa_cli` result does NOT mean features
  are unavailable — it means a different access path is needed.

### Capability investigation meta-note

Recurring pattern in this codebase's hardware investigations: a
single negative diagnostic test gets interpreted as "feature not
supported," when actually the feature IS supported via a different
access path. Examples encountered:

- NAN initially missed because `iw list | grep modes` doesn't show
  it (it's not an interface mode); confirmed via `iw phy`
- WiFi P2P initially missed because `wpa_cli p2p_find` errored on a
  control socket issue; confirmed compiled in via binary `strings`,
  later confirmed not driver-exposed via D-Bus introspection
- Modem capabilities initially missed because ModemManager held the
  modem down with no SIM; confirmed working via direct QMI when
  modem manually forced online
- Direct phone-to-phone radio initially conflated with "ad-hoc mode"
  when AP-hosted is functionally equivalent for two-device snap
  exchange

**Mitigation pattern for future investigations:** when asking
"does this hardware support X," run at least 3 independent probe
methods before concluding negative. Check binary `strings`, kernel
declarations (`iw phy`, `/proc/net/*`), D-Bus introspection, and
direct API tests. Single-tool negatives are weak evidence.

This applies to future WiFi modes, modem features, BLE capabilities,
and any other "what can this radio do" question that comes up.

### Crash diagnostic commands (reference)

**[FIXED V32.34+] Shutdown segfault after "Network stats summary at
shutdown".** Symptom: `[1] segfault (core dumped)` immediately after the
end-of-session NetworkStats summary line in main.cpp; happened even with
session 0 ms / 0 peers (fast quit on a profile that had stored snaps from
a previous session). Root cause: ThumbnailService worker jobs (the inbox's
async thumbnail loader, 2-thread QThreadPool) deliver results to the GUI
thread via `QMetaObject::invokeMethod(ctx, lambda, Qt::QueuedConnection)`.
At `return rc` in main, `window` (→ InboxWidget) and then `app`
(QApplication) destruct, then static locals destruct (ThumbnailService
last). A job completing during/after that teardown posted a queued event
to a thread whose event dispatcher was gone (QCoreApplication::instance()
already null) → deref of a destroyed dispatcher → segfault. The QPointer
context guard only protected the target object, not the dead event loop.
Fix (src/ui/ThumbnailService.cpp + .hpp): added `std::atomic<bool>
m_shuttingDown` set true at the very top of `~ThumbnailService` (before
waitForDone), and the worker checks BOTH `m_shuttingDown` AND
`QCoreApplication::instance()` before posting; if either says we're tearing
down, it drops the result (the cache was already populated, so a relaunch
still benefits). Device-verify: quit immediately after the inbox renders
thumbnails — should exit cleanly, no core dump.

### Crash diagnostic commands (reference)

For when crashes happen, run these immediately on the affected device:

```bash
# Kernel-level signal traces (segfaults, oom-kill, etc.)
sudo dmesg -T | grep -iE 'snapclone|segfault|killed|oom|trap' | tail -50

# systemd-coredump captures (if installed — usually is on pmOS)
coredumpctl list | tail -5
coredumpctl info <PID>        # details
coredumpctl gdb <PID>         # interactive (needs debuginfo)

# Journal slice around the crash
journalctl --since "1 hour ago" --no-pager | grep -iE 'snapclone|coredump|segfault|abort'
journalctl --since "2026-05-15 13:50:00" --until "2026-05-15 13:55:00" --no-pager
```

### Camera diagnostic commands (reference)

For the "rear camera shows front camera" issue. Run on the phone:

```bash
# Is PipeWire actually running?
systemctl --user status pipewire pipewire-pulse wireplumber

# What does PipeWire see?
pw-dump | grep -B 1 -A 5 -i 'media.class\|virtual'
pw-cli list-objects Node | grep -B 1 -A 3 -i 'camera\|video\|virtual'

# What does the kernel expose?
v4l2-ctl --list-devices

# Camera bridge running?
ps aux | grep -i 'cam'
systemctl --user list-units | grep -i cam

# Does gst-launch work in isolation with what SnapClone tries?
gst-launch-1.0 -v pipewiresrc target-object=virtual-cam-rear \
  ! videoconvert ! autovideosink

# With debug to see WHY it fails
GST_DEBUG=3 gst-launch-1.0 -v pipewiresrc target-object=virtual-cam-rear \
  ! videoconvert ! autovideosink 2>&1 | tail -40
```

### TOFU manual test recipe (reference)

To verify the S-20 mismatch branch (since logs only exercise the
match branch):

```bash
# 1. Quit SnapClone fully first.
# 2. Back up DB.
cd ~/.local/share/SnapCloneProject/SnapClone/
cp snapclone.db snapclone.db.backup

# 3. Inspect pubkeys (pick a contact to corrupt).
sqlite3 snapclone.db \
  "SELECT peer_id, substr(hex(public_key),1,16), updated_at FROM peer_public_keys"

# 4. Corrupt one peer's stored key (simulates attacker presenting
#    different pubkey).
sqlite3 snapclone.db \
  "UPDATE peer_public_keys SET public_key = randomblob(32) WHERE peer_id = 'sc-051d7c06'"

# 5. Restart SnapClone. Have the OTHER device connect.

# 6. Log on this device should show:
#    PUBKEY CHANGED for sc-051d7c06 — recorded as pending; existing key kept.

# 7. Open Settings → Security. Should show a card with fingerprints.

# 8. Inspect pending row directly:
sqlite3 snapclone.db \
  "SELECT peer_id, substr(hex(new_public_key),1,16), \
          substr(hex(old_public_key),1,16), detected_at, attempt_count \
   FROM pending_key_changes"

# 9. Test Reject — pending row should be deleted, peer_public_keys
#    unchanged (the real Alice's key kept).

# 10. Re-corrupt, this time test Accept — pending should be deleted,
#     peer_public_keys updated to the corrupted-random key.
#     (You'll then need to restore from backup to undo, or have the
#     real other device re-SHAK which will now trigger another mismatch.)

# 11. Restore:
cp snapclone.db.backup snapclone.db
```

---

## 🧪 Needs investigation

### Audio normalization + speech-boost accessibility (design brief, V32.34+)

**Motivation.** Today the recorder's audio branch is bare:
`autoaudiosrc ! queue ! audioconvert ! audioresample ! opusenc ! mux.`
— raw mic level, no normalization. Real-world snaps have wildly
inconsistent loudness (windy outdoors vs quiet room vs phone held at
arm's length), and quiet speech is hard to hear on playback,
especially for users with hearing loss or on a phone speaker in a
noisy place. We want (a) consistent perceived loudness across snaps
and (b) an accessibility "speech boost" that makes voice intelligible
even when it's quiet relative to background.

**Two distinct problems, two mechanisms:**

1. **Loudness normalization (always-on, capture-side).** Bring every
   recording toward a consistent target loudness so playback volume is
   predictable. The broadcast standard is EBU R128 / LUFS integrated
   loudness (-16 LUFS is a good mobile target). GStreamer has no direct
   R128 normalizer in the base set, but two practical options:
     * **Two-pass (transcode-side):** during the existing
       VideoTranscoder pass, run `ffmpeg`-style loudnorm if ffmpeg is
       present (it does true R128 two-pass). We already shell out for
       transcode; adding `-af loudnorm=I=-16:TP=-1.5:LRA=11` is one
       flag. Downside: depends on ffmpeg being installed.
     * **One-pass (capture-side):** insert `audiodynamic`
       (compressor mode) + a fixed makeup gain in the record pipeline.
       Not true LUFS but cheap and real-time. `audiodynamic
       mode=compressor characteristics=soft-knee ratio=0.5
       threshold=0.5` + `audioamplify amplification=N`. Tune against
       recorded samples. This is the pragmatic default since it needs
       no extra dependency.

2. **Speech boost (opt-in, accessibility setting).** Distinct from
   normalization: detect that speech is present and selectively raise
   ITS level relative to non-speech background. Two sub-parts:

   a. **Speech presence detection (VAD).** Cheap, well-understood:
      * Energy + zero-crossing-rate VAD is ~20 lines and runs on the
        raw PCM — enough to gate "is someone talking right now."
      * WebRTC's VAD (in libwebrtc, exposed by the `webrtcdsp`
        GStreamer element's voice-activity flag) is more robust and
        we may already have libwebrtc on the system for the
        `webrtcdsp` echo/noise path. Prefer it when present.
      * A small Rust crate `snapclone-audio` could host an energy/ZCR
        VAD + a band-energy speech-likelihood score (speech sits
        ~300 Hz–3.4 kHz; compute the ratio of in-band to total energy
        per 20 ms frame). Returns a 0..1 "speech-ness" per frame —
        mirrors how `snapclone-saliency` scores landmark candidates.
        Reuses the FFI-with-opaque-handle house pattern.
        **BUILT (V32.34+).** `rust/snapclone-audio` exists: stateless
        `sc_audio_speech_likelihood()` + stateful detector
        (`sc_audio_vad_new/_process/_free`, opaque handle, adaptive
        noise floor + continuous bandpass biquad + smoothed output).
        Blends short-time energy, ZCR, and a speech-band energy ratio
        (cheap RBJ bandpass biquad, no FFT). 8/8 calibration tests pass
        (silence→0, in-band tone beats rumble + hiss, stateful
        roundtrip climbs on speech + falls on silence). Built + linked
        via `SNAPCLONE_RUST_AUDIO` (default ON), FFI header at
        `include/audio/snapclone_audio_ffi.h`. **Wired into playback:**
        VideoSnapViewerDialog's `fileContainsSpeech()` decodes a ~3s
        head segment to mono S16LE PCM, runs it through the VAD, and
        only inserts the speech-boost EQ when speech is detected
        (gated on QSettings `audio/speechBoostAutoOnly`, default true).
        Still TODO: webrtcdsp-when-present path; full per-frame
        in-process gating (current is a one-shot head probe).

   b. **Selective boost.** When speech-ness is high, apply extra gain
      to the speech band. Cleanest real-time approach in GStreamer:
      `audiowsincband`/`equalizer-nbands` to lift the 300 Hz–3.4 kHz
      band, gated by the VAD flag. Or simpler: a dynamic compressor
      tuned so quiet speech is brought up while loud background is
      held down (de facto speech emphasis since speech is the
      transient signal). The accessibility setting controls the boost
      amount (Off / Mild / Strong) and persists to
      QSettings `audio/speechBoost`.

**Where it lives.**
  * Capture-side normalization + speech boost → insert into the
    VideoRecorder audio branch (the `audioconvert ! audioresample`
    chain). Element availability gated by `gst-inspect` probes like
    the camera GPU path (`audiodynamic`, `webrtcdsp`, `equalizer-nbands`
    are all in gst-plugins-good/bad — may not be installed on minimal
    pmOS; fall back to plain passthrough + makeup gain).
  * Playback-side speech boost → **DOWN-PAYMENT DONE (V32.34+).**
    VideoSnapViewerDialog's gst-launch playback pipeline now inserts an
    optional `equalizer-3bands` between `audioresample` and
    `autoaudiosink`, gated on QSettings `audio/speechBoost`
    (off/mild/strong, default off) and on `equalizer-3bands` being
    installed. Mild = mid +4dB / low −2dB; Strong = mid +8dB / low −4dB
    (mid band ≈1.1kHz where speech intelligibility lives, low band trims
    rumble/handling noise). This is the preferred design: record clean +
    normalized, boost on playback per the viewer's accessibility
    preference, so others' copies are never affected. **Still TODO:**
      - Wire the Settings → Accessibility UI control (the QSettings key
        works now; no UI surface yet — set it manually for testing).
      - Apply the same segment on the Qt-media playback fallback path
        (currently only the gst-launch path has it; Qt path has no easy
        EQ insertion — may need a QAudioBufferOutput filter or just
        document that boost requires the gst backend).
      - VAD-driven dynamic boost (only lift when speech is actually
        present) per the snapclone-audio crate sketch below — the
        static EQ is the v1; dynamic gating is the v2.
      - **Clipping at high volume — FIXED V32.34+.** The bare EQ lift
        pushed loud-speech peaks past 0 dBFS → hard clip → audible
        crackle. Fixed with the standard boost-and-limit chain:
        pre-attenuate (`volume`, ~70% of the boost in dB) → EQ lift →
        soft-knee limiter (`audiodynamic` compressor, threshold 0.85,
        ratio 0.3) to round off residual peaks instead of square-wave
        clipping. (Note: an FFT-domain boost, the originally-suggested
        approach, would clip identically and add overlap-add windowing
        artifacts — headroom + limiter is the correct fix, not more
        frequency bins.) Limiter is element-availability-gated; without
        audiodynamic the pre-attenuation alone still helps.

**Accessibility framing (important).** Speech boost should be a
first-class accessibility setting, not buried in video options:
  * Settings → Accessibility → "Speech boost on playback" (Off /
    Mild / Strong) with a one-line explanation ("Makes spoken words
    louder and clearer on received videos").
  * Respect the OS-level accessibility hint if one exists (Plasma
    Mobile exposes some via the a11y D-Bus; check
    `org.a11y.Status`). If the user has system-wide "enhance speech"
    or mono-audio set, default our boost to Mild.
  * The boost is per-VIEWER and applies at playback, so it never
    degrades what's sent to others — a deaf-friendly default doesn't
    make everyone else's copy sound processed.

**Open questions:**
  * Does FP5 pmOS ship `webrtcdsp`? If yes, VAD + noise suppression +
    AGC come essentially free and we should prefer that whole chain.
    If not, the energy/ZCR Rust VAD is the portable fallback.
  * Real-time cost of band EQ on the A55 cores during playback —
    measure before defaulting Strong on.
  * Two-pass loudnorm needs the full file first; fine for transcode
    (already a post-record pass) but can't be real-time at capture.
    Decide: normalize at transcode (accurate, ffmpeg-dependent) vs
    compress at capture (cheap, always-available). Probably BOTH —
    capture-side compressor as the floor, transcode-side loudnorm as
    the polish when ffmpeg is present.
  * Interaction with Opus's own built-in DTX/VBR — make sure our
    compressor isn't fighting the encoder's silence handling.

**Estimate:** 1 session for the capture-side compressor + makeup gain
(no new deps, immediate consistency win), 1 for the `snapclone-audio`
VAD crate + playback-side speech boost + the Accessibility settings
panel, 1 for device tuning + the webrtcdsp-when-present path.


- [x] ~~**Self-originated punches from 127.0.0.1 with our own selfId**~~
  **Fixed V32.34+.** Recurring every ~20s (maintenance punch interval),
  caught harmlessly by the `PUNCH-LOOPBACK-DROP-V2` receive-side guard
  but spamming a warning forever. The earlier hypotheses (zombie second
  instance, stale cached 127.0.0.1) were wrong. Actual root cause: when
  two peers sit behind the SAME CGNAT they report the IDENTICAL
  reflexive (public) IP — so a peer's connreq hands us OUR OWN public
  address (e.g. 69.156.66.68:9876) as the punch target. We then
  maintenance-punch our own public IP every 20s; the kernel hairpins
  the packet back as 127.0.0.1 and the receive guard drops it. The
  send-side `initiateHolePunch` guard only refused literal loopback
  (127.0.0.1/::1/localhost) + matching selfId, NOT our own reflexive
  host — so the self-punch toward our public IP slipped through. Fix:
  added an `isSelfReflexive` check (host-only compare against
  `m_reflexive.host`) to the same guard. Now we refuse the doomed
  self-punch at the source and emit holePunchFailed, which cleanly
  routes to relay (the path that was already succeeding for these peers
  in the logs). No wasted packet every 20s, no recurring warning.
- [ ] When two devices come online "simultaneously" (within 5s)
  on same WiFi, sometimes DHT lookup finishes before gossip catches
  the LAN address. Result: try public IP first (timeout) before LAN.
  The same-LAN-first fix in V28 should help once cached, but the
  *first* connection from cold-start still hits this.
- [ ] On Fedora desktop, `qt.qpa.services` portal warning:
  `"Could not register app ID: App info not found for 'snapclone'"`.
  Probably needs a `.desktop` file installed for portal recognition.
- [ ] Memory profile under sustained snap exchange — is the per-snap
  encryption buffer being freed?
- [ ] Why does `pendingDhtLookups` sometimes get stuck (`lookup
  already in flight` repeats for 60s+)? The 60s timeout should clear
  it but apparently doesn't always.
- [ ] CGNAT detection logs are informational only — should they
  trigger an immediate relay-preference upgrade?
- [ ] **Rear camera shows up as front on FP5.** Confirmed across
  multiple builds. The `m_mirrorPreview` detection in CameraWidget
  flags it as front-facing because some part of the device-id
  string matches "front" or "tracking" — but the actual sensor is
  the rear one. Likely the camera-bridge is publishing the rear
  sensor's PipeWire node with a name that contains "front" in
  some role/description field, OR the bridge's discovery picks
  the wrong sensor as "rear" and the actual rear ends up labeled
  ultrawide/auxiliary. Investigation steps:
    1. `pw-dump | grep -B2 -A5 libcamera_input` on the FP5 to see
       the exact node names and `node.description` fields
    2. `cam --list` to see how libcamera classifies each sensor
       (look for `back`/`front`/`external` markers in `Location`
       field on libcamera 0.0.5+)
    3. Compare to `SnapClone-side device-id selection logic in
       CameraWidget::onSelectorChanged — the substring match
       (`"front"` or `"tracking"`) is fragile if the bridge
       names things differently
  Likely fix lives in the Rust bridge's `pick_camera_nodes_multi`
  (camera-bridge/src/capture.rs:~302) — use libcamera's location
  metadata, not heuristic name matching.

  **V32.34+ partial — fallback hardening.** The `cam --list`
  (libcamera Location) path was already the primary classifier and is
  correct; the swap bugs lived in the KEYWORD fallback used when
  `cam --list` is unavailable. Hardened `pick_camera_nodes_multi`
  (camera-bridge/src/capture.rs): (1) **rear==front collision break** —
  if keyword matching + compiled defaults resolved both facings to the
  SAME PipeWire node (the "swap button does nothing" symptom on devices
  whose descriptions lack facing words), the front is now reassigned to
  another probed node by exclusion (clean on 2-camera devices; a logged
  guess on 3+); (2) **default validation** — compiled defaults are now
  checked against the probed set and the mismatch is logged loudly rather
  than silently routing a dead node; (3) **per-role exclusivity** — a
  description carrying BOTH "front" and "back" can no longer claim both
  roles; (4) `pick_via_cam_list_multi` now rejects its own result and
  falls through to the keyword path if it somehow produced rear==front
  (resolve_node substring collision). Added a `#[cfg(test)]` module
  covering distinct-role assignment, the 2-camera tie-break, the
  single-camera tolerated-collision case, and front/back keyword
  classification. STILL OPEN: the FP5-specific case where `cam --list`
  IS available but reports a Location the user disagrees with (the rear
  sensor genuinely labeled differently by libcamera firmware), and the
  SnapClone-side `m_mirrorPreview` / CameraWidget substring match that
  decides mirroring independently — those need device-side `cam --list` /
  `pw-dump` output to resolve correctly rather than guessing.
- [ ] **Piggyback photo storage on contacts' devices for offline-gap
  fill.** Conceptual goal: snaps can travel through the network even
  when sender and receiver aren't online simultaneously, by storing
  encrypted copies on trusted contact devices that ARE reachable to
  both at different times. Up to ~5 hops deep. Like a slow-network
  store-and-forward overlay built on the friend graph.

  **High-level shape:**
    - Each snap encrypted end-to-end (already true via existing
      `CryptoService`). The piggyback host can store the blob but
      can't read it.
    - Routing decision: when sender can't reach recipient directly,
      check if any *common contact* is currently online. If yes,
      hand the blob to that contact's device with a delivery
      receipt expectation. That contact stores it until either:
      (a) the recipient comes online and asks for it, or (b) the
      contact reaches the recipient first and delivers.
    - Up to 5 hops: if no common contact is online, hand to a
      contact's contact (friend-of-friend), and so on. Hops are
      authenticated — each hop signs an entry in a chain so the
      eventual recipient can verify the path.
    - Aging: blobs expire after the snap's TTL regardless of
      delivery state.

  **Open questions (the "your call on this number / certificate
  chains / proof of pathway" items the user flagged):**
    - **Hop depth:** user said "your call." 5 is a reasonable max
      because graph-diameter studies of friend networks consistently
      land around 4-6 for arbitrary-pair reachability. Set as a
      compile-time const, possibly user-tunable. Below 3 the gap-fill
      benefit is marginal; above 7 the trust-chain audit cost gets
      bad.
    - **Trust chain verification:** each hop signs an entry
      `(prev_hop_id, my_id, signed_at_ms, blob_hash, sig)`. The
      chain is appended-only; later hops can't rewrite earlier
      entries because each entry signs over the previous entry's
      hash (like git commits). Recipient verifies the chain at
      delivery time: all signatures valid, no loops, hop count
      under limit.
    - **"Proof of pathway" crypto:** the user's term for the chain
      above. Equivalent to a directed-acyclic delivery receipt
      where each node attests "I received this from X and
      forwarded to Y." This is well-studied — it's basically
      onion-routing's path verification without the onion layers
      (since payload encryption is end-to-end, not per-hop).
    - **Graph loop avoidance:** chain must not revisit a node. Each
      hop checks the existing chain before accepting. Loop attempt =
      drop with log.
    - **Storage budget:** each peer caches up to N MB of piggyback
      blobs. LRU eviction by signed_at_ms. Suggest 100MB default
      with a settings slider. Recipient receipt deletes the blob
      across the chain via a small `DELIVERED` broadcast.
    - **Privacy considerations:**
        - Hop nodes see ciphertext only — they DO learn that A→B
          want to communicate (timing analysis is possible). For
          users who want stronger anonymity, route through hops
          that ALSO see the sender's normal traffic (so piggyback
          traffic blends in).
        - Friend graphs are visible to hops within the chain: hop N
          knows the previous and next hop. Doesn't see the full
          chain. Hop-chain rotation prevents long-term observation
          patterns.
        - The recipient learns the FULL chain at delivery. This is
          intentional (verification needs it) but means contacts
          can deduce other contacts' availability patterns from
          chain participation.
    - **Sybil resistance:** chain only validates if every hop is
      a real contact of someone in the friend graph. New random
      nodes can't insert themselves mid-chain.
    - **Bandwidth fairness:** outbound piggyback bandwidth capped
      to N% of available, so the relay role doesn't degrade the
      hosting user's experience. Capped per-peer to prevent one
      busy peer monopolising relay capacity.

  **Why not in V1:** this is a real protocol design that interacts
  with the existing snap pipeline, retransmit logic, DHT lookups,
  and storage manager. Several months of work to do safely. File
  as a long-term design direction; revisit when basic delivery
  is stable.

  **What we can do TODAY toward this end without committing:**
    1. Make sure the existing snap encryption is opaque enough that
       a hypothetical relay couldn't extract metadata beyond
       "sender_id → recipient_id, blob_size, timestamp." (Mostly
       already true via wire format v3 — verify.)
    2. Design the chain-entry signature format compatible with
       existing signing primitives in CryptoService.
    3. Add a "delivery receipt" message type even before the relay
       exists; useful for direct delivery confirmation too.

---

## 🌱 Long-term directions

*Filed for future consideration. Not actionable today. Items here pass
two tests: (a) they fit SnapClone's architecture and values, and
(b) we have at least a rough technical sketch of how they'd work.
They're queued for when the foundational work (security rework,
performance baseline, transport convergence) has stabilized enough
to take on additional surface area.*

### Weather alert relay (cell broadcast bridging)

Receive **cell broadcast** alerts (weather warnings, AMBER alerts,
presidential alerts) from the device's modem and relay them to nearby
SnapClone users over BLE / LAN / mesh transports. The bridge value:
during disasters when cell infrastructure is patchy, alerts can
propagate through the mesh and reach users who have lost carrier
coverage but still have nearby SnapClone peers.

**Why this fits the project:**

- Cell broadcast reception works on SIM-less modems (no carrier auth
  required). So a device whose carrier has failed can still receive
  alerts, then forward them via the mesh transports SnapClone already
  builds for snap delivery.
- Alerts from official sources (NWS, EU-Alert, J-Alert, etc.) are
  explicit public-domain data in most jurisdictions — re-broadcasting
  is allowed.
- The audience overlap is real: users who care about
  resilient-comms-when-infrastructure-fails are the same audience
  for the BLE highways and proximity-witness work.

**Technical references:**
- Cell broadcast wire format: 3GPP TS 23.041 (well-documented standard).
- Android API: `CellBroadcastService` (system-privileged on stock
  Android, accessible on AOSP / postmarketOS / GrapheneOS variants
  that expose it).
- postmarketOS: ModemManager exposes cell broadcast via D-Bus on
  some modem stacks; needs per-modem testing.

**Design constraints (when implementing):**

- **Don't claim official status.** UI must distinguish "received
  from NWS, relayed via SnapClone, time T" from "Official NWS alert
  in your area." Users need to know they're getting a relayed copy
  that may be stale. Forwarding chain should be visible.
- **Preserve the original timestamp.** Alerts go stale fast (tornado
  warning expires in ~30 min). Forwarding a 60-minute-old tornado
  warning is worse than not forwarding at all — at minimum, mark
  visibly as expired or refuse to forward past TTL.
- **Per-jurisdiction rules differ.** Verify the public-domain status
  of alert sources per country before assuming. EU-Alert, J-Alert,
  KBPS Wireless Emergency Alerts — each has its own legal posture.
- **Cap forward hops or TTL.** Without a cap, a stale alert could
  loop the mesh indefinitely. Each forwarded alert decrements a hop
  counter or expires at the original alert's expiry time, whichever
  comes first.
- **Don't deduplicate by content alone.** Two genuine alerts for the
  same event (issued, then updated 5 min later with refined coords)
  should both reach users. Dedup on (alert-id, issue-time) pair.

**Things to NOT do here** (filed for clarity, not as items):

- Don't relay user-generated emergency content with this mechanism
  (that's the rejected mesh-routed-911 territory; see "🚫 Decided NOT
  to do").
- Don't aggregate or analyze the alerts (e.g. "12 weather warnings
  in your area" derived dashboards). Just relay verbatim. Analysis
  introduces an editorial role we don't want.

**Effort estimate when picked up:** Multi-week. Modem cell-broadcast
plumbing is OS-specific and varies per build; on Linux+ModemManager
it's reachable but needs testing per modem chipset. The forwarding
+ UI display layer on top is straightforward once the source feed
works. Should be its own design doc (`CELL_BROADCAST_RELAY.md`) before
implementation, similar to how `DHT_DISCOVERABILITY.md` preceded the
DHT-discoverability work.

### LoRa transport via standalone Meshtastic node

Long-range (km-scale) low-bandwidth (sub-kbps to ~5 kbps) radio for
SnapClone snaps when no other transport reaches. This is the first
transport-direction entry with actual hardware in motion (user
prototyping an HT-CT62-based dongle; off-the-shelf standalone
Meshtastic nodes also viable and arguably better).

**Recommended architecture: standalone Meshtastic node, not USB dongle.**

Off-the-shelf battery-powered Meshtastic devices (typical config:
nRF52840 + SX1262, e-ink display, Qi charging, 5000-10000 mAh battery)
run stock Meshtastic firmware. SnapClone phone speaks the Meshtastic
BLE API to the local node. The node handles the LoRa radio side and
mesh routing.

This is strictly better than USB-dongle approach because:
- Self-powered (no drain on phone battery)
- BLE link instead of USB-OTG (works while phone is in pocket;
  works on multiple platforms without OTG support questions)
- Multi-user capable (the node serves whoever's in BLE range)
- Standalone operation (node keeps relaying even when no phone
  is connected, contributing to the broader mesh)
- Already-shipping hardware ($30-80 range, no custom build)

**Implementation path:**

1. **Phase 1 — Plumbing (~1 week).** SnapClone implements the
   Meshtastic BLE protocol: protobuf-encoded messages over the
   defined BLE service UUID. Sends "hello," receives node info,
   enumerates mesh peers. Reference implementations in the
   Meshtastic Android/iOS apps to study.

2. **Phase 2 — SnapClone over Meshtastic (~1-2 weeks).** Define a
   SnapClone-specific message type that rides as a Meshtastic data
   packet. Fragment SnapClone snaps into Meshtastic-sized payloads
   (~230 bytes useful per packet). Reassemble on receipt. Map
   Meshtastic node IDs to SnapClone sc-ids via a discovery exchange.

3. **Phase 3 — Bandwidth-aware UX (~1 week).** Decide which snap
   types route via Meshtastic. Build queue + retry logic for slow
   delivery. UI affordances ("sending via mesh, ETA ~3 minutes").

**Total realistic estimate: 3-4 weeks for usable v1.**

**Bandwidth reality (LoRa physics, not device-specific):**

- LoRa SF7 (fastest, shortest range): ~5 kbps usable
- LoRa SF11 (Meshtastic default, balanced): ~1 kbps usable
- LoRa SF12 (slowest, longest range): ~250 bps usable

Per-snap times at typical Meshtastic config:

- Text snap (~140 bytes): seconds
- Compressed thumbnail (5 KB): 30-90 seconds
- Full photo (200 KB): 10-40 minutes (impractical without
  explicit user consent and progress UI)
- Video: not feasible on this transport

The right framing is "text and thumbnails work, full photos are a
slow stretch users explicitly opt into, videos are out." NOT
"alternative full transport." This affects UX design: SnapClone
needs a transport-aware send flow that warns or restricts when
Meshtastic is the only available path.

**Bonus interop:**

If other Meshtastic users exist in radio range, SnapClone packets
ride their relay infrastructure for free. They cannot decode the
SnapClone payload (it's wrapped in a generic Meshtastic data packet
type) but they DO forward it. This means SnapClone gets the benefit
of the broader Meshtastic mesh community without that community
needing to do anything.

**Things to NOT do here** (filed for clarity, not as items):

- Don't write custom firmware for the LoRa device (months of work;
  Meshtastic already exists and is well-supported on common
  hardware including HT-CT62-class modules and the nRF52840+SX1262
  combo)
- Don't try to ride Meshtastic mesh for high-bandwidth content
  (will starve the channel for other users; rude to the community
  whose infrastructure you're using)
- Don't claim "SnapClone works over LoRa" as a primary feature
  (audience is genuinely small; this is best framed as
  "infra-dropout resilience" or "field deployment" niche)

**Effort note:** Hardware path is already underway (user has
HT-CT62 module on order, plans USB-C dongle with internal fractal
antenna). Off-the-shelf Meshtastic nodes are a faster path to
working hardware (1 week vs months for custom build). Both can
inform the SnapClone-side integration; recommendation is to develop
SnapClone integration against off-the-shelf hardware while custom
hardware effort continues in parallel.

### Infra-dropout resilience via nearby-peer transports

(Already partially captured above; consolidating here as the
overarching direction this work serves.)

The motivating use case for several pending transport directions:
**SnapClone should deliver snaps to nearby peers (sub-10km) when
normal internet infrastructure is unavailable or degraded.**

Specific scenarios:
- Public WiFi is captive-portal-broken or hostile
- Cellular has weak signal or carrier issues
- Both ends are in an area without internet (event venue, remote
  location, disaster zone)
- User wants to bypass potentially-hostile public infrastructure
  for privacy reasons

Current available paths (sub-300m):
- BLE for proximity discovery (already in scope, BLE highways
  direction)
- WiFi via existing infrastructure (already works)

Direct mode (sub-300m, deliberate session):
- AP-hosted hotspot pair (one phone hosts via NetworkManager,
  other joins via SSID convention) — confirmed working on
  FP5/pmOS, see platform reality entries

Medium-range (~5-15km, low-bandwidth):
- LoRa via Meshtastic node (above)

Out of scope today (require new hardware or new physics):
- Direct phone-to-phone WiFi at >300m (limited by phone radio
  design)
- Satellite messaging (no consumer APIs available today)
- Cellular without carrier (regulatory)

The architecture commitment: SnapClone's transport-abstraction
layer should make adding new transports straightforward as they
become viable. Don't tightly couple any specific transport to
application logic. When new physics or new hardware arrives, the
integration is a new transport, not a redesign.

### Direct Mode (peer-to-peer hotspot for nearby snaps)

User-initiated direct connection between two SnapClone phones
without depending on existing WiFi infrastructure. Implementation
grounded in empirically-confirmed hardware capability: AP+client
architecture (one phone hosts a hotspot via NetworkManager, other
joins via SSID convention).

**Why this is first long-term direction with a clear "build this in
N weeks" plan:** unlike LoRa (waiting for hardware), Meshtastic
(implementation phase scoping), or weather alerts (needs design doc
first), Direct Mode uses only confirmed-available capability on
current hardware. No new transports, no new dependencies.

**User flow:**

1. Alice wants direct connection to Bob nearby
2. Alice taps "Direct connection" in SnapClone, picks Bob from
   contacts, picks host or accepts auto-arbitration
3. Alice's phone creates an ephemeral AP (NetworkManager hotspot
   command)
4. SSID convention: `SC-Direct-<short-id>` so other SnapClone
   instances recognize it
5. Bob's SnapClone scans for `SC-*` SSIDs, finds Alice's, prompts
   Bob to join
6. Bob's phone connects to Alice's AP via DHCP, lands on local
   subnet (typically 10.42.0.0/24)
7. SnapClone's existing gossip discovery finds Bob's phone on the
   subnet
8. Snaps deliver via direct UDP/TCP, existing crypto stack handles
   encryption end-to-end (don't rely on open WiFi)
9. Session ends: Alice tears down AP, both phones return to
   previous network state

**Auto-arbitration for who hosts** (when both phones could):
- Recommended: deterministic hash-based (lower sc-id hash hosts).
  Trivial, predictable, no coordination needed.
- Alternative: BLE side-channel negotiation (more robust but
  requires BLE highways infrastructure)

**Trade-off worth surfacing in UX:** hosting phone temporarily
disconnects from internet (the WiFi radio can't simultaneously be
AP and client). Cellular still available if SIM present. UX should
warn: "Direct Mode will pause your WiFi internet during the
session."

**Implementation effort:** 1-2 weeks for MVP. AP creation already
works via nmcli; SnapClone needs the SSID convention, scan-and-join
UI, session teardown logic.

**Motivation (re-stated from infra-dropout direction):**
- Bypass potentially-hostile public WiFi (captive portals,
  blocked ports, MITM risk)
- Reliability during infra dropout
- No carrier dependency
- Privacy: no third-party network in the path

This direction is buildable today against confirmed-working hardware
capability. Highest-leverage transport direction by feasibility.

### Bluetooth / ad-hoc "pointing" triangulation (design brief, V32.34+)

**Goal.** Help two people physically find each other ("which way do I
walk?") using only the radios already in the phone — no GPS fix
required, works indoors. A coarse direction + distance estimate is
enough: "your contact is roughly that way, ~15m." Complements the
Wayfinder page, which today needs a location fix for both parties.

**Signals available without extra hardware:**
  * **BLE RSSI ranging.** Received signal strength → coarse distance
    (path-loss model). Noisy (±several meters, multipath indoors) but
    monotonic enough for "warmer / colder" as you move. Both phones
    advertise + scan a SnapClone BLE service UUID carrying a rotating
    ephemeral id derived from the pair's session key (so only the
    intended contact resolves it; everyone else sees noise).
  * **Direction via motion, not antennas.** Phones don't expose
    per-antenna phase (no AoA without special hardware / BLE 5.1
    direction-finding, which FP5 likely lacks). So derive bearing the
    cheap way: as the user walks/turns, correlate RSSI change with the
    compass heading + accelerometer step events. RSSI rising while
    facing 040° → contact is roughly NE. This is "pointing" by
    gradient-following rather than true triangulation — a few seconds
    of movement converges on a bearing arrow.
  * **Multi-anchor triangulation (when ≥3 references exist).** If
    several SnapClone peers (or known-location landmarks with BLE
    beacons) are in range, trilaterate position from their RSSI
    distances. Degrades gracefully: 3+ anchors → position; 1 anchor →
    range-only "warmer/colder"; with motion → bearing arrow.

**Proposed shape:**
  * New `core/ProximitySource` QObject: owns the BLE advertise+scan
    (QtBluetooth `QBluetoothDeviceDiscoveryAgent` +
    `QLowEnergyController` peripheral role), emits
    `proximityUpdate(UserId, rssi, estDistanceM, confidence)`.
  * A small Rust `snapclone-proximity` crate (house FFI pattern) does
    the math: RSSI→distance path-loss with a per-device calibration
    constant, a Kalman/EMA smoother over the noisy RSSI stream, and
    the gradient-following bearing estimator that fuses RSSI deltas
    with compass + step events. Pure f32, no deps — mirrors
    snapclone-saliency / snapclone-audio.
    **BUILT (V32.34+).** `rust/snapclone-proximity` exists: opaque
    estimator handle (`sc_proximity_new/_update/_free`), log-distance
    path-loss ranging, EMA smoothing, coarse display bands
    (VeryClose/Close/Near/Far), and the gradient-following bearing
    estimator (credits the heading faced when RSSI improves, decaying
    bins, confidence from accumulated evidence). 8/8 calibration tests
    pass (stronger RSSI → closer, 1m at txPower, noisy-RSSI smoothing,
    bearing converges toward the improving heading). Built via
    `SNAPCLONE_RUST_PROXIMITY` (default ON), FFI at
    `include/proximity/snapclone_proximity_ffi.h`. **NOT yet linked to
    a C++ consumer** — needs the `core/ProximitySource` Qt/BlueZ object
    (BLE advertise+scan) to feed it RSSI + compass heading. The crate
    is the math foundation; the BlueZ plumbing is the remaining work.
  * Wayfinder gains a "nearby / pointing" mode: when a contact is in
    BLE range but neither party has a usable GPS fix, show a bearing
    arrow + distance band ("very close / ~10m / ~30m") instead of the
    map. Tap to start the few-seconds "walk to calibrate bearing"
    flow.

**Open questions / risks:**
  * QtBluetooth peripheral (advertising) support on pmOS / BlueZ —
    confirm the FP5 stack allows simultaneous advertise+scan. Some
    BlueZ configs restrict peripheral role.
  * Privacy: rotating ephemeral ids per pair (derived from session
    key + a coarse time bucket) so SnapClone users aren't trackable by
    a static BLE id. Never advertise the stable sc-id.
  * Battery: BLE scan is cheap-ish but continuous scan isn't free.
    Gate on an explicit "find my contact" action + a timeout, not
    always-on. Ties into the SleepState machine (only scan in Active).
  * Calibration: RSSI→distance needs a per-model constant; ship a
    default for FP5 and let it self-calibrate when a GPS fix IS
    available (correlate known distance with observed RSSI).

### "Carrier pigeon" delay-tolerant Bluetooth highway (design brief, V32.34+)

**Goal.** A SECOND, deliberately-slow transport for the store-and-
forward piggyback overlay (see "Piggyback photo storage" above) that
uses short-range radio (BLE / Bluetooth Classic / WiFi-Aware) as the
physical link. Where the IP piggyback path needs both a hop and the
next hop to be online and internet-reachable at the same time, the
carrier-pigeon path exploits *physical proximity over time*: phones
that come near each other — on a train, in an office, at a café —
silently exchange queued encrypted blobs, which then ride that person
to the next encounter. Classic delay-tolerant networking (DTN /
"pocket-switched network"), the way Firechat, Briar, and the original
"data mules" work. Latency is minutes-to-hours, not milliseconds —
hence "carrier pigeon" — but it needs zero infrastructure and survives
total internet outage.

**Why it's a distinct protocol, not just a transport swap:**
  * The IP piggyback assumes a routing decision *now* ("is a common
    contact online right now?"). The pigeon path makes NO routing
    decision — it's epidemic/opportunistic: hand a copy to anyone you
    physically meet who's willing to carry it, bounded by trust + TTL.
  * It must be **store-at-rest**: blobs sit encrypted on the carrier's
    disk for the gap between encounters (could be hours). The existing
    `CryptoService` end-to-end encryption already means carriers can't
    read them; the pigeon layer adds an at-rest queue with TTL +
    storage budget + dedup.
  * Delivery is **probabilistic**, so it needs anti-flooding: a blob
    shouldn't replicate unboundedly across every phone in a city.
    Bound by (a) hop count from the piggyback chain, (b) TTL, (c) a
    Bloom-filter "summary vector" exchange so two meeting phones only
    transfer blobs the other lacks (epidemic-routing's standard
    handshake), (d) per-carrier storage cap with LRU.

**Proposed shape (layers on top of existing piggyback crypto):**
  1. **Encounter detection:** reuse the ProximitySource BLE scan
     above. When a SnapClone peer is in stable range for >N seconds,
     open a transfer link (BLE GATT for small blobs; negotiate up to
     Bluetooth Classic / WiFi-Aware for large video snaps — BLE alone
     is ~1-2 KB/s, far too slow for a video, so BLE is the *discovery
     + summary-vector* channel and a faster radio carries the payload).
  2. **Summary-vector handshake:** each side sends a compact digest
     (Bloom filter or sorted blob-hash list) of what it's carrying +
     what it's waiting for. Transfer only the set difference. This is
     the bandwidth-saver that makes epidemic routing tractable.
  3. **At-rest queue:** new `storage` table `pigeon_queue(blob_hash,
     ciphertext_path, recipient_id, hop_chain, ttl_ms, received_ms,
     bytes)`. Reuses the piggyback chain-of-custody signatures for
     trust. LRU + TTL eviction. Storage-budget slider (default 100MB,
     shared with the IP piggyback cache).
  4. **Delivery + cancellation:** when a carrier finally meets the
     recipient (or a better-connected hop), it delivers and emits the
     `DELIVERED` receipt that propagates back through the chain to
     evict the blob everywhere — same mechanism as IP piggyback.

**The "slow Bluetooth highway" framing:** model it explicitly as a
transport with a *cost/latency tier* far below IP. The
TieredDeliveryPolicy (already scaffolded for thumbnail-then-better)
extends naturally: a snap can be sent over IP if reachable now, AND
queued to the pigeon highway as a fallback that activates only if IP
delivery hasn't confirmed within some window. The two races; first
delivery wins; the loser's copies get the DELIVERED eviction.

**Open questions / risks:**
  * **Payload radio choice on FP5/pmOS:** BLE is discovery-only
    (too slow for video). Bluetooth Classic RFCOMM (~1-2 Mbit/s) or
    WiFi-Aware/NAN (faster, but BlueZ/pmOS support uncertain) for the
    bulk transfer. Confirm what the FP5 stack actually supports;
    fall back to the existing WiFi-Direct AP transport (already filed)
    as the "fast pigeon" when both phones can spin it up.
  * **Abuse / flooding:** an attacker floods the pigeon network with
    junk blobs. Mitigations: only carry blobs whose hop-chain includes
    at least one of YOUR contacts (web-of-trust gating), per-sender
    rate limit, proof-of-work stamp on blob creation, storage cap.
  * **Battery:** opportunistic transfer must be cheap and bounded.
    Only transfer on a stable encounter + on charger OR above a
    battery threshold (ties into PowerManager / SleepState). Never
    let the pigeon layer keep the radio hot in the user's pocket all
    day — that's the exact battery sink we just fixed for the camera.
  * **Privacy / timing analysis:** carriers learn that a blob exists
    and its (encrypted) recipient routing token. Rotate routing
    tokens; consider k-anonymity batching so a carrier can't pin a
    single blob to a single recipient.
  * **Relationship to the IP piggyback overlay:** these should SHARE
    the chain-of-custody crypto, the storage budget, and the DELIVERED
    eviction. The pigeon layer is "piggyback, but the link is physical
    proximity over time instead of simultaneous internet reachability."
    Build the shared piggyback core first (it's already specced), then
    the pigeon layer is mostly the encounter-detection + summary-vector
    + slow-radio transfer on top.

**Estimate:** large. The shared piggyback core is its own multi-session
project; the pigeon layer adds the BLE/proximity work (shares the
ProximitySource above) + the slow-radio bulk transfer + the at-rest
queue. Sequence: (1) ProximitySource + snapclone-proximity crate
[shared with the pointing/triangulation feature], (2) piggyback core
[already specced], (3) summary-vector + at-rest queue, (4) slow-radio
bulk transfer. Each is a distinct deliverable; the pointing/
triangulation feature in the brief above is the natural first step
because it's useful on its own and builds the BLE foundation the
pigeon highway needs.

---

## 🚫 Decided NOT to do

- ❌ Web client (would force a server back into the architecture)
- ❌ "Stories" / public feeds (vision conflict — engagement-loop)
- ❌ Stranger discovery / "find new friends" (P2P means you already
  know who you want to talk to)
- ❌ Snapchat-style screenshot notifications (vision conflict —
  treating recipients as suspects)
- ❌ Per-environment-specific commits to repo (Cisco openh264 install,
  v4l2loopback setup, Bell router port forwarding) — these are user
  environment concerns, not app concerns

- ❌ **Mesh-routed automatic 911 calling.** SnapClone surfacing
  emergency information to nearby contacts and offering them a
  "call 911 on Alice's behalf" button is fine (bystander pattern,
  the user makes the call from their own phone). What is NOT in
  scope: app-initiated or mesh-routed emergency calling where
  SnapClone makes the dispatch decision or uses another user's
  phone to place the call automatically. Reasons:
    - Becomes emergency call infrastructure with associated legal
      obligations (PSAP routing contracts, location accuracy
      mandates, 24/7 SLAs, audit trails)
    - Weaponizable: an attacker who can inject SnapClone messages
      could cause innocent users' phones to dial 911 with false
      reports (mesh-distributed swatting)
    - Liability when relay fails: legal exposure for wrongful
      death, FCC enforcement, criminal negligence in some
      jurisdictions
    - "Best effort community project" is not a defense for
      emergency services failures
  If revisited, requires: legal counsel review for the specific
  design, PSAP partnership, abuse model with realistic mitigations,
  product liability insurance, acceptance of operational uptime
  responsibility. Don't reopen without those.

- ❌ **Mandatory identity retention / subpoena-able accountability
  layer.** Proposed variants include: users retain identifying
  data (face hash + photo) and produce on demand, contacts retain
  it, government holds it, mandatory ID registration to use the
  network. All variants share the same fatal incompatibility:
    - E2E encryption and state-level accountability are
      fundamentally incompatible (decades of crypto policy debate
      has reached this conclusion; cf. "Going Dark" debates)
    - SnapClone's architecture (P2P, no central entity,
      contact-only, encrypted) is incompatible with mechanisms
      requiring central retention or state submission
    - The user base SnapClone serves (privacy-needing users,
      dissidents, journalists, abuse victims) would be HARMED by
      such mechanisms, not protected
    - "Bad actors will exist" is the Signal-position values
      commitment of building E2E P2P privacy tools; the answer is
      contact-level + community-level defenses, not state-level
  Bad-actor defenses that DO fit the architecture: contact-only
  reachability (already implemented), block functionality,
  proximity-witness web-of-trust (filed as future direction),
  community moderation primitives (also future). State-mediated
  accountability is closed off as a direction.

- ❌ **Direct AX.25 transport in SnapClone-core.** Multiple
  rounds of exploration converged on: legal complexity (Part 97
  encryption prohibition, ambiguity on hashes, steganography
  concerns on images, signature legality unclear), bandwidth
  limits (1200-9600 bps unsuitable for snap content), lossy
  transport requires multi-packet protocols that increase
  metadata exposure, audience tiny (hundreds globally at most),
  architectural mismatch (SnapClone's core trust model relies on
  encryption). Recommended alternative: third-party bridge as a
  separate project by someone in the amateur community who handles
  their specific use case and legal review. SnapClone-core stays
  out of the amateur radio transport space.

- ❌ **Unauthorized use of cellular spectrum.** Includes any
  scheme using cellular bands without carrier permission: custom
  protocols on cellular frequencies, modem firmware modification
  to transmit non-protocol data, unauthorized power levels, etc.
  Cellular bands are closed off by regulation in essentially all
  jurisdictions, not just by convention. The modem hardware is
  capable; the regulatory framework is not. The cellular radio
  is one of SnapClone's input sensors (read modem-side
  diagnostics, location info, signal quality), and one of its
  output channels when carrier-authorized (normal cellular IP);
  it is NOT a channel SnapClone can repurpose for arbitrary
  transmission. This closes the whole class of "but what if we
  used the modem differently" explorations.

- ❌ **Unencrypted "walkie-talkie" fallback mode.** A UI mode
  where users send unencrypted snaps when no normal network is
  available. Rejected because:
    - Trades the privacy guarantee for marginal convenience
    - Gets used exactly when privacy matters most (urgent
      messages, network compromise scenarios)
    - Creates new attack incentives (attacker degrades network to
      force fallback)
    - Conflates "no encryption allowed" scenarios (AX.25 on
      amateur) with "no network at all" scenarios (different
      problems, different solutions)
  Better alternative: graceful loud failure. Show queued snaps
  prominently, surface "tried via: [paths attempted, all failed],
  estimated delivery [time or unknown]," offer to compose
  SMS-fallback message via the system SMS app so user falls back
  through a DIFFERENT tool, explicitly, knowing it's not
  SnapClone-private. SnapClone itself stays encrypted.

- [ ] **Face-aware bandwidth optimization** — see
  `docs/FACE_BANDWIDTH.md` for the full design. Three phases:

  **Phase 1 (DONE V32.32):** local face crop library with consent
  gating, per-label LRU caps, and total byte budgets. Implemented
  in `snapclone-face::library`. Still needs C++ wiring to capture
  and inbox paths (separate turn).

  **Phase 2 (skeleton V32.32, real impl pending):** opportunistic
  delta compression. Sender picks a reference crop from local
  library, encodes new snap as `(reference_hash, codec_delta)`,
  recipient applies delta to local copy of same reference. Falls
  back to full image when reference unavailable or quality
  insufficient. Skeleton in `snapclone-face::reference`. Real
  impl pending decisions on:
    - Codec (WebP-Ref vs AVIF-Ref vs custom)
    - Perceptual hash implementation (need a canonical decode
      path for library crop bytes first)
    - Wire format extension fields for snap header
    - Negotiation protocol (one-extra-RTT vs proactive-both-send
      vs cached-receiver-state)

  **Phase 3 (DEFERRED):** on-device personal neural model. Only
  if Phase 2 measurably helps. Open questions:
    - Training framework (`burn` is the obvious pick)
    - Backbone (MobileNetV2 transfer-learning vs custom)
    - Update cadence / when training runs (overnight while
      charging? after N samples?)
    - Cross-device determinism strategy if the model output ever
      crosses device boundaries

  **Cold start safety net:** entire system is opportunistic. Day
  one: every snap sends as full image. Over weeks: library grows;
  compression kicks in occasionally. Failure modes (model
  uncertain, reference missing, decoder error) all degrade
  cleanly to "send full image" — recipients never see degraded
  face reconstruction.

  **Privacy posture:** all biometric data is user-originated
  (selfies + crops from incoming snaps), stays on-device, wiped
  on consent withdrawal. The `embedding` and `identity` modules
  in `snapclone-face` are deferred — they were scaffolded for a
  generic-pretrained-model direction that's been superseded by
  this approach.

  Direction confirmed by user in V32.32 brainstorm: "ok to improve
  network traffic not to be super great so cold start is fine
  because we fall back to sending full image."

---

## Notes for the next session

When picking up after a break, check `journal.txt` in
`/mnt/transcripts/` for the most recent session summaries. The
transcripts include full reasoning trails for non-obvious decisions.

If a bug seems new, check the relevant `Q_LOGGING_CATEGORY` first
(`snapclone.app`, `snapclone.nat`, `snapclone.dht`, `snapclone.router`,
`snapclone.camera`, `snapclone.gossip`, etc.) — most issues surface
through specific log channels and `journalctl --user -t SnapClone`
filtering by category narrows things down fast.

### V32.34+ camera smoke test checklist

What's new since the last device test:
- `CameraPolicy` + `CameraPolicyStore` (timeouts/limits, QSettings-backed)
- `CameraTester` + `CameraHealthService` (probe-mode backend, cached results)
- `GstLaunchBackend` framedrop watchdog + early-exit stderr translation
- `CameraRegistry` cam --list authoritative facing + multi-rear
- Settings → Camera section (devices, tuning, diagnostics)

Logging categories of interest (turn on with `QT_LOGGING_RULES`):
- `snapclone.camera.registry.debug=true` — pw-dump + cam --list parsing
- `snapclone.camera.gst.debug=true` — gst-launch lifecycle
- `snapclone.camera.tester.debug=true` — probe driver
- `snapclone.camera.health.debug=true` — health service decisions

Quick smoke sequence:
1. **Cold start, open camera tab**: viewfinder should appear within
   `firstFrameTimeoutMs` (5s default). Log should show
   `First frame from <node> size=480x640`.
2. **Open Settings → Camera section**: device list shows badges
   (Untested initially). After a beat: "Test all" probes run
   serially, badges flip to `working (NxM, Tms)`. The one currently
   used by the viewfinder shows immediately as `working (NxM, 0ms)`
   without needing a probe (live-session in-use mark).
3. **Tap "Test" on a single device**: probes only that one. Badge
   reflects result. In-use device's button is short-circuited
   (returns Working synthetic).
4. **Change Capture width spinbox to 720**: edit persists to QSettings.
   Restart viewfinder → log shows `First frame ... size=720x...`.
5. **Switch from Rear to PIP (Rear+Selfie)**: log shows main backend
   stop+start on rear + PIP backend start on front. Both should
   reach first frame; in-use markers should NOT collide.
6. **Stop camera (navigate away)**: log shows backend stopped(),
   `setDeviceInUse(<id>, false)`. No probes pile up.

Failure modes to watch for:
- "framedrop watchdog: no data for Nms" — preview froze, fired
  the new stall detector. Camera kernel driver hung; needs
  airplane-mode toggle on FP5.
- "PipeWire node <name> not found" — bridge died or sensor
  unplugged. Error message translation kicked in.
- Settings page badges stay Untested forever — `CameraTester`
  not getting `firstFrameReceived` from probes. Likely a backend
  issue (gst-launch failure) rather than tester issue.

### Environment variables (V32.32)

Camera bridge:
- `SC_CAM_FLIP_REAR` — videoflip method for rear cam (default
  "counterclockwise"). Valid: none, clockwise, rotate-180,
  counterclockwise, horizontal-flip, vertical-flip,
  upper-left-diagonal, upper-right-diagonal, automatic.
- `SC_CAM_FLIP_FRONT` — same for front (default "clockwise").
- `SC_CAM_FLIP_REAR_ULTRAWIDE` — same for ultrawide (defaults to
  SC_CAM_FLIP_REAR if unset, then to compiled default).
- `SC_USE_GPU=1` — use `glupload ! glcolorconvert ! gldownload`
  for YUV→RGB conversion instead of CPU `videoconvert`. Requires
  GStreamer's GL plugins (probed at startup; see bridge logs).
  Opt-in because GL plugin availability varies. Quality identical;
  this is purely a CPU/battery optimization.

Video recorder:
- `SC_USE_HW_ENCODER=1` — use `v4l2h264enc` (SoC hardware video
  accelerator) instead of x264enc/openh264enc/vp8enc. Big battery
  win when it works, but on some pmOS kernels Venus produces corrupt
  streams. Verify on your hardware before enabling permanently.

Camera bridge node overrides (rarely needed; auto-discovery usually works):
- `PW_NODE_REAR`, `PW_NODE_FRONT`, `PW_NODE_REAR_ULTRAWIDE` —
  override the discovered PipeWire node names.
- `MODEL_PATH` — path to YOLO face-detect model (default
  "models/face.onnx").
- `DETECTION_INTERVAL`, `ZOOM_FACTOR`, `OUTPUT_WIDTH`,
  `OUTPUT_HEIGHT` — tracking pipeline tunables.

## Battery: efficiency-core thread placement (V32.34+)
  - **E-CORE THREAD PLACEMENT for battery — classification + policy crate BUILT
    & TESTED; C++ apply deferred.** USER ASK: anything we can keep on E-cores to
    help battery (FP5 big.LITTLE; also laptops — Linux has schedulers, pinning,
    uclamp). FINDING: no affinity/nice/sched code existed; background threads run
    wherever the scheduler puts them (e.g. waking a big core for a 55-min DHT
    re-announce). The fix: classify E/P cores, demote genuinely-background work.
      * RUST CRATE snapclone-coresched (pure-std, no deps, no core dep → testable;
        crate-type staticlib+rlib). topology: parse /sys/.../cpu_capacity (1024 =
        max; lower = efficiency) + cpufreq max-freq FALLBACK → E/P split; uniform
        CPU reports !heterogeneous so callers skip pinning. policy: WorkClass
        {Interactive, LatencySensitive, PeriodicBackground, BackgroundCompute,
        BackgroundIo} → Placement {mechanism (None/NiceOnly/UclampPreferEfficiency
        /AffinityEfficiency), cpu_set, nice_delta, uclamp_max}. uclamp for bursty
        periodic work (can still migrate), HARD affinity for sustained background
        compute (keep off P-cores the UI wants), nice always (orthogonal,
        universal fallback). C FFI sc_coresched_has_efficiency_cores +
        sc_coresched_placement (caller applies the syscall). **15 tests pass**
        (FP5 4+4 big.LITTLE, Intel hybrid, uniform-degrades-to-nice-only, freq
        fallback, per-class policy, FFI). VERIFIED: release libsnapclone_coresched
        .a builds; include/coresched/snapclone_coresched_ffi.h passes
        gcc -fsyntax-only as C AND C++. CMake SNAPCLONE_RUST_CORESCHED option +
        build block added. Doc docs/CORE_SCHEDULING.md (thread→class table).
      * UPDATE (continuation): FIXED an FP5-SPECIFIC classification BUG. The
        original "within 5% of max = performance" rule was wrong for the FP5's
        QCM6490, which is THREE-tier: 4 silver A55 (~335), 3 gold A78 (~910), 1
        prime A78 (1024). 910 < 0.95·1024, so the OLD rule put the 3 gold A78
        big cores into EFFICIENCY alongside the A55s, leaving only the single
        prime core as "performance" — it would have hard-pinned background work
        onto real big cores AND starved the UI to one core. REPLACED the fixed
        tolerance with GAP DETECTION: sort distinct capacities, split at the
        largest RELATIVE gap requiring ≥1.3× (A55→A78 is 2.7×; gold→prime is only
        1.13×). Now the 4 A55s are efficiency and ALL 4 A78s are performance.
        Public API unchanged (internal logic only) → C++ FFI + header unaffected,
        release staticlib still builds. Added fp5_qcm6490_three_tier +
        near_uniform_does_not_split_on_noise tests; **17 tests pass** now.
      * THREAD→CLASS MAPPING (confirmed against real code): PeriodicBackground =
        DHT reannounce (55min) + network-change poll (2min) + WS keepalive (30s) +
        pollinator beacon; BackgroundCompute = ThumbnailService pool (2 threads) +
        AmbientPaletteDeriver + face detection + video post; BackgroundIo =
        FirewallPortManager ufw/nft shell-outs; LatencySensitive (NOT demoted) =
        CDAF focus sweep + VCM control (viewfinder depends on speed); Interactive
        = UI thread.
      * DEFERRED (needs live threads + syscalls, untestable here): C++
        applyPlacement(tid, CPlacement) calling sched_setaffinity / sched_setattr
        (SCHED_FLAG_UTIL_CLAMP) / setpriority with EPERM as a soft skip; tagging
        each background thread/pool with its WorkClass at creation; linking the
        staticlib in the same change as the first caller (no dead link); optional
        settings toggle to disable demotion. The classification + decision (the
        portable, get-it-wrong-and-waste-battery part) is fully tested; the apply
        is thin per-thread syscall glue.

  - **CAMERA BATTERY AUDIT — screen-off / out-of-focus power (V32.34+). Fixes
    shipped, brace-checked, unverified on device (no Qt here).**
    USER ASK: make sure the "blurred frame screensaver" on the camera page saves
    battery when out of focus or screen off. FINDING (honest): there is NO
    feature literally named "blurred frame screensaver" — the camera page's idle
    visual is the AmbientBlobs overlay (already well-behaved: 10fps timer that
    stops when backgrounded/inactive/hidden). The REAL power behavior is mostly
    sound: screen-off stops the sensor+ISP pipeline (m_backend->stop — the
    dominant FP5 draw), PiP, and the 1Hz landmark/scene timers; page-hide stops
    the camera after a 250ms debounce; app-background drops frame processing via
    shouldProcessFrames().
    BUT found a GENUINE GAP: shouldProcessFrames() returned true for m_scanMode
    (QR) BEFORE checking screen state, so a QR scanner left open when the screen
    turned off kept processing frames (QR decode / blurHash / transforms) against
    a dark screen — wasted battery. FIXES: (1) shouldProcessFrames() now returns
    false on m_screenOff FIRST, overriding scan mode (nothing to scan for or show
    while dark). (2) screen-ON resume now restarts the backend for (m_widgetVisible
    || m_scanMode), not just visible — without this a scan that survived a
    screen-off stayed dead after unlock. (3) screen-off/on handler now calls
    updateProcessingState() so the PAUSED/RESUMED diagnostic reflects the
    screen-off gate. Brace-checked; m_screenOff/m_scanMode init confirmed.
    NOTE: unverified on device (no Qt). The dominant power cost (sensor+ISP) was
    already correctly stopped on screen-off; this closes the scan-mode-while-dark
    leak and a scan-resume bug. No new "screensaver" was added — the existing
    idle visual (AmbientBlobs) already pauses correctly.

  - **WINDOW-CLOSE SEGFAULT — VideoSnapViewerDialog::teardownQtMedia missed the
    closingDown() gate (V32.34+). Strong candidate, NOT confirmed — needs the
    backtrace.** USER REPORT: segfault when a window closes, "some sort of state"
    left behind; flagged it resembled a PRIOR fix but said don't assume same cause.
    AUDIT (didn't guess-patch): the prior shutdown-segfault fix added a
    QCoreApplication::closingDown() gate to FOUR QProcess teardowns
    (GstLaunchBackend, VideoRecorder, VideoTranscoder, VideoSnapViewerDialog::
    teardownGst) — verified all four STILL present (no regression). But found a
    MISSED SIBLING: VideoSnapViewerDialog::teardownQtMedia() (the Qt-multimedia
    playback path, parallel to teardownGst) still used BARE deleteLater() on
    m_player (QMediaPlayer) + m_audioOut (QAudioOutput) with NO closingDown gate.
    Same hazard class: the dtor calls teardownQtMedia() then teardownGst(); at app
    exit deleteLater() never fires (loop won't pump again), so the QMediaPlayer
    (holding a backend pipeline + threads + file handles) is torn down
    inconsistently during static destruction → exactly the "state left when a
    window closes" segfault. This matches the user's "same solution but maybe new"
    framing: same FIX SHAPE, a site the prior pass didn't cover.
    FIX: mirror teardownGst's gate — disconnect() all signals first, then on
    closingDown() delete DIRECTLY (loop won't pump; signals disconnected so no
    media callback can re-enter), else keep deleteLater() (loop pumps; avoids a
    synchronous delete re-entering a media-backend callback). Applied to BOTH
    m_player and m_audioOut. setVideoOutput(nullptr) already detaches from the
    QVideoWidget before delete (no write-to-freed-sink). Brace-checked;
    QCoreApplication already included.
    HONEST STATUS: this is a STRONG candidate that fits the symptom + matches the
    proven prior pattern, but it is NOT confirmed root cause (per the user: don't
    assume). CONFIRM with a backtrace: on crash, coredumpctl gdb (or lldb -c
    <core>) → bt. If the crashing frame is in QMediaPlayer/QAudioOutput/Qt
    multimedia teardown or static destruction, this is it. If it's elsewhere
    (DHT worker thread, a different dialog, the unified-nav gesture commit), this
    fix is still CORRECT (removes a real latent shutdown hazard) but isn't THE
    bug — send the bt and which window was closing. Also useful: does it crash on
    closing the VIDEO viewer specifically, or other windows too? (Other dialogs
    audited — QrScanDialog/SnapComposerDialog/ContactPickerDialog/SnapViewerDialog
    have no QMediaPlayer/QProcess/gst teardown, so they're not this pattern.)

  - **FIELD TEST FINDINGS (V32.34+, desktop M2 Ultra + about to test mobile).**
    User ran the test guide. RESULTS:
      * ✓ Camera battery: appActive→false PAUSES frame processing (the
        phone-in-pocket-on-main-page case the user actually cared about);
        camera-list correctly collapses front+rear+PiP → none on a camera-less
        system; scanner failover rear→front→clean-exit works; camera-off on
        leaving page already worked.
      * ⚠ STALE CHECKOUT, not a bug: user's `cargo test` showed
        snapclone-debounce + snapclone-av1policy "0 passed" (binaries recompiled
        fresh but ran 0 tests). Verified HERE the test code IS present (debounce
        351 lines, mod tests @215, 10 tests; av1policy mod tests @201, 9 tests)
        and re-running passes 10/10 + 9/9. Cause: user's local copy of those two
        crate dirs predates the tests. FIX (user side): copy the latest
        rust/snapclone-debounce/ + rust/snapclone-av1policy/ over the checkout.
        videosrc/saliency/coresched/group/microdht all passed on their machine.
      * ✗ USB WEBCAM NOT PICKED UP ON DESKTOP — real gap, now partially closed.
        Root cause confirmed by inspection: CameraRegistry only matches
        virtual-cam-* + libcamera_input.* node names; a desktop USB/UVC webcam is
        a generic Video/Source (api.v4l2) node it filtered out. This is the
        deferred "pw-dump classification" item, now hit on hardware. FIX (Rust,
        TESTED): added classify_pw_node() + PwNode to snapclone-videosrc — given
        a pw-dump node's (object_id, node_name, media.class, api, description) it
        decides SourceKind (UsbWebcam / PhysicalCamera / ScreenCast) + a capture
        group (each USB device its OWN group, offset +1000 so it never collides
        with the FP5 ISP group 0 → two webcams run simultaneously). Skips FP5
        nodes (registry still owns those), ignores audio/sinks. **15 videosrc
        tests pass** (+6) incl usb_webcam_is_recognized (the exact failing case)
        + two_usb_webcams_are_independent. DEFERRED (C++, no Qt/PipeWire here):
        wire CameraRegistry's pw-dump pass to call this classifier for the
        generic nodes it currently ignores + surface them in the camera list.
      * ⚠ NO P/E-CORE LOGGING ON DESKTOP — EXPECTED, not a regression. The C++
        applyPlacement syscall layer (sched_setattr UCLAMP / setpriority /
        affinity) is still DEFERRED/UNWIRED; only the Rust classification+policy
        exist and nothing in C++ calls them or logs yet. User's cpupower confirms
        the M2 Ultra IS heterogeneous (CPUs 0-3,12-15 @2.42GHz = E; 4-7,20-23
        @3.26GHz = P) so the gap-detection classifier will classify it correctly
        once wired. User notes uclamp is the nicer API + the Rust side already
        works in their audio stack on the same laptop → confidence the apply step
        will land cleanly. NEXT: write applyPlacement + tag the background threads
        (untestable here; needs live threads).
      * PIP TWO-MENU UI (user request, design): the PiP selector becomes TWO
        menus — (1) MODE: detect webcam / multicam / rear+front / screenshare;
        (2) LAYOUT: the combination of source slots. The snapclone-videosrc model
        already backs this: classify_pw_node enumerates the available sources +
        kinds, and compatible_secondaries(all, main) answers "what can be the 2nd
        simultaneous source" to populate menu 2 (excludes the ISP sibling on FP5,
        includes independent webcams + screenshare). DEFERRED: the C++ two-menu
        widget + wiring compatible_secondaries into it.

  - **MOBILE FIELD TEST (V32.34+) — scroll/overscroll tuning DONE (brace-checked,
    unverified on device); "no snaps received" PENDING the two device pastes.**
    USER (two FP5s on same Bell wifi): headline = NO SNAPS RECEIVED; cameras
    "improved a bit but still shaky"; tap-to-drag carries over pages but feels
    unintuitive + direction reversed; "huge rubber band even when not dragging
    hard past the end."
    SCROLL/OVERSCROLL TUNING (src/ui/SwipeNavigator.cpp + include): applied the
    user's four asks. (1) MORE FRICTION: kOverscrollRubberK 0.4→0.22 (smaller
    sqrt output per raw pull). (2) BLENDED DEADZONE: new overscrollDeadzone(size)
    = max(kOverscrollDeadzonePx=12, size×kOverscrollDeadzoneFrac=0.02) subtracted
    from |raw| BEFORE the rubber-band curve → zero visible overscroll below it
    (kills "huge band when barely past end" + bounce-on-near-zero-motion); blend
    of fixed-px + viewport-fraction respects HiDPI/LoDPI as requested. (3) ACCEL
    FACTOR: kOverscrollAccel=1.35 exponent on the post-deadzone pull sharpens
    onset. (4) tightened kOverscrollMaxRatio 0.3→0.22. CRITICAL CONSISTENCY: the
    spring-back INVERSE formula (grab-during-springback) updated to match the new
    forward curve: |raw| = (offset²/(v·k))^(1/accel) + dz — else a re-grab would
    jump. DIRECTION: page-drag sign verified CORRECT (m_dragDistance = pos.x −
    start.x; drag-left → next page — standard). The "reversed" feel is the
    SUB-TAB DETENT on touch; flipped its DEFAULT (ui/scroll_detent_invert_touch
    false→true) to content-follows-finger (mobile platform norm), still
    user-overridable; mouse/generic defaults unchanged. qPow/qSqrt via <QtMath>
    (already included). Brace-checked both files. UNVERIFIED on device.
    USER'S BIGGER IDEA (deferred, noted): add expanding QML components at top+
    bottom of each page and drive flick/rubber-band/snap via those + scroll — a
    more QML-native gesture surface than the current widget event-filter physics.
    Larger refactor; captured for later.
    NO SNAPS RECEIVED — NOT YET DIAGNOSED (need the pastes). The two device logs
    referenced weren't included in the message. Receive path: routeOutbound →
    encryptAndSend → peer connects via gossip discovery (Gossip JOIN seen) then
    WS/UDP on port 9876; inbound via routeInbound → handleV3Message →
    inboundReady. For two peers on the SAME wifi the expected path is LAN-direct.
    DIAGNOSTIC PLAN when pastes arrive — localize to one of:
      (a) DISCOVERY: does each device log the OTHER as a peer after "Gossip JOIN"?
          (grep peerDiscovered / "[INBOUND] Awaiting approval for new peer").
      (b) CONNECT: does WS/UDP to :9876 succeed, or STUN/punch loop with no LAN
          direct? (the desktop log showed STUN timeouts + no UPnP/NAT-PMP — on
          LAN that shouldn't matter, but if it's trying to round-trip via DHT/
          STUN instead of direct-LAN that's the bug).
      (c) HANDSHAKE: "SHK2 responder: finishHybridHandshake failed" or "Decrypt
          failed from" → crypto/identity mismatch.
      (d) OFFER/DATA: "OFFER sent for" on sender but no "v3 ANNOUNCE|DATA" /
          inboundReady on receiver → offer/answer or data-channel stall.
    Most likely on same-wifi: discovery succeeds but the two never form a direct
    LAN connection (firewall on :9876, or the connect logic preferring a
    reflexive/DHT path that doesn't complete). The pastes will say which.

  - **snapclone-gesture — REUSABLE two-tier gesture MODULE extracted. Rust-tested
    (15), staticlib + C header verified. NOT yet wired back into SnapClone.**
    USER: "the touchpad small distance for subpage loop and big swipes for page
    jumps feels amazing — make that a reusable module for other apps if possible."
    Audited SwipeNavigator: the feel-good part (detent accumulator + refresh-rate
    rate budget + arm-window re-arm + fire-once-per-streak + post-fire takeover
    lock + distance/velocity page-tier test) is PURE scalar/time logic; the Qt
    bits (QScrollBar/QWheelEvent/widgetAt/viewport translate, update(),
    currentSubTabbed) are plumbing AROUND it. Clean separation → extractable.
    NEW CRATE rust/snapclone-gesture (ZERO deps, crate-type staticlib+rlib):
    TwoTierGesture state machine + GestureConfig (defaults mirror the app's tuned
    constants: detent_threshold 220, arm_window 1000ms, rate 0.75 px/ms,
    commit_fraction 0.22, flick 0.30 px/ms & 14px, takeover_lock 350ms). API:
    on_scroll(delta, now_ms)->Outcome{None|DetentFired{dir}|DetentProgress
    {fraction,edge}}, reset_streak(), on_release(vel, drag)->ReleaseOutcome
    {Stay|PageJump{dir}}, locked(now). App owns toolkit bits (read events, is-
    at-limit check, render barrier, do the move); module owns the decisions.
    **11 core tests** incl small_sustained_scroll_fires_one_detent,
    one_sustained_scroll_does_not_rip_through_subpages (lock+rate cap runaway),
    refresh_rate_normalization_240hz_vs_60hz_match (the high-refresh bug, now
    fires at same wall-time), arm_window_lapse_rearms_fresh, page-jump by flick
    AND by distance, commit_fraction_scales_with_page_size. C FFI (ffi.rs):
    opaque ScGesture* handle, sc_gesture_new / new_config / on_scroll (out
    fraction+dir) / reset_streak / on_release / locked / free; panic-safe
    (catch_unwind→ERROR) + null-tolerant; **4 FFI tests** (lifecycle, release
    jump, null handle, null out-params). Header include/gesture/
    snapclone_gesture_ffi.h (SC_GESTURE_* codes) passes gcc -fsyntax-only as C
    AND C++. README.md documents Rust + C usage + how to drop into another app.
    VERIFIED: 15 tests pass + release libsnapclone_gesture.a builds — but ONLY
    STANDALONE (crate has zero deps; in-workspace `cargo test -p` hits the
    pre-existing getrandom/edition2024 drift from unrelated sibling crates, so
    test via a temp copy or once the workspace toolchain is bumped). Added to
    rust/Cargo.toml workspace members. HONEST: the feel is hand-validated, not
    test-validated (tests prove decisions correct/stable; thresholds may need
    per-app tuning). NOT wired back into SnapClone — the navigator keeps its own
    tuned inline copy; migrating it to call this crate is a separate
    behavior-preserving refactor (deferred).

  - **LAPTOP LOG REVIEW (V32.34+) — corrected diagnosis + a real sub-tab scrub
    fix. C++ wired, brace-checked, unverified on device.**
    CONTEXT CORRECTION: the earlier "no snaps received" + DHT-timeout→relay
    fallback was NOT a bug — user confirmed the two peers (sc-051d7c06,
    sc-7c271ed3) were taken OFFLINE and they were testing on the laptop alone. So
    DHT lookups timing out and escalating DHT→Magic-Wormhole relay is CORRECT
    behavior reaching genuinely-unreachable contacts; no LAN peer to gossip with.
    Dropped the connection-path theory (would've been chasing a phantom). [The
    DirectDiscovery subnet-match observation stands as a NOTE for the real
    two-device case later: LAN probe needs a cached LAN addr per peer and only
    learns it from the DHT — gossip JOINs aren't populating m_knownPeers with a
    usable LAN addr, so two same-subnet peers may never go direct. Revisit when
    actually testing two live devices.]
    CONFIRMED WORKING from this log: Plasma shell lock → appActive=false →
    Frame processing PAUSED, sleepstate→Background, clean (19:47 cycle). Qt camera
    backend on desktop (FaceTime HD), record→transcode→ROI all fired: "ROI plan:
    2 directive(s) — bg penalty + face boost" + the libsvtav1 CRF32 hierarchical
    cmd ran and produced a 316KB AV1 from a 3.97MB mkv (the AV1+ROI path works
    end-to-end on desktop; size 316KB for ~8.7s 480x640 — a real datapoint).
    REAL UX BUG FOUND in the wheel/scroll log: repeated "[HANDOFF] sub-tab scrub
    WRAPPED off start → breaks to page" spam (19:45–19:46, ~a dozen times). Root
    cause: the horizontal scrub-commit path called cycleSubTab (WRAPPING) — at
    the first sub-tab a back-scrub teleported the row to the LAST sub-tab, and the
    log claimed "breaks to page" while the early return actually changed NO page
    (misleading message + jarring teleport = the "unintuitive at edges" feel).
    Meanwhile the VERTICAL detent path already did it right via advanceSubTab
    (non-wrapping) + edge-spill-to-page. FIX (src/ui/SwipeNavigator.cpp ~1737):
    horizontal scrub now uses advanceSubTab; at the first/last sub-tab it SPILLS
    to the adjacent page (bounds-checked) exactly like the detent path, or
    resists+bounces if there's no page to spill to — no wrap-around. cycleSubTab
    retained but now UNUSED (private member, no warning); header doc corrected.
    Brace-checked. VERIFY on device: the "WRAPPED off start" lines should be GONE,
    and scrubbing left on the first sub-tab should either go to the previous page
    or gently resist — never jump the row to the far end.

  - **CAMERA TILING + LIVE-PREVIEW PERF — analysis + tested geometry module +
    staged GPU(Vulkan/Mesa) plan. Geometry Rust-verified; GPU glue DEFERRED.**
    USER: keep analyzing the tiling fix + improve live preview perf, "maybe GPU
    accel via vulkan + mesa." Separated into TWO distinct problems:
    (1) TILING = correctness: row STRIDE mismatch (producer pads each row to an
    alignment boundary; naive reader assumes tight width×bpp → diagonal skew).
    KEY LOG FACT: at 1280×720 bytes-per-frame=3686400=1280×720×4 EXACTLY → NO
    padding at that width, so tiling only appears at non-alignment-clean widths.
    The pipeline already parses negotiated sink caps and passes the real
    bytesPerLine when present (the correct fix); residual risk is the pre-caps
    window / producers that don't emit stride.
    (2) PERF = the GPU path does glupload→…→gldownload→videoconvert→fdsink then
    builds a QImage per frame — i.e. it DOWNLOADS the GPU result back to system
    memory (3.7MB/frame at 720p) + pipe + heap copy. The GPU accel is half-wasted
    on the CPU round-trip.
    SHIPPED + TESTED: snapclone-videosrc::geometry module — round_up,
    padded_stride, frame_size, tight_frame_size, infer_stride (deterministic core
    of the old inline "Stride candidates:" heuristic), best_stride_guess (auto-
    recovery, not just diagnostic), align_width_down (avoid padding entirely),
    StrideCandidate. **5 tests** incl tight_1280_has_no_padding (matches the
    device log), odd_width_pads_and_tiles, infer_recovers_the_alignment_from_a_
    bloated_buffer, align_width_down_kills_padding. **20 videosrc tests total.**
    This replaces the UNTESTED inline heuristic in GstLaunchBackend with verified
    math, and is the shared geometry both the tiling fix and the GPU texture-
    import layout need.
    DESIGN DOC docs/CAMERA_GPU_AND_TILING.md — staged GPU plan cheapest-first:
    (A) prefer v4l2convert m2m HW converter ahead of GL for pure format convert;
    (B) in-process GStreamer appsink (kills the subprocess + fd pipe, keeps
    stride meta intact); (C) THE BIG WIN — zero-copy DMA-BUF → Qt RHI / Vulkan-
    on-Mesa import: appsink on memory:GLMemory/DMABuf → export DMA-BUF fd →
    QRhiTexture::createFrom (Qt 6.6+) / QSGSimpleTextureNode, or Vulkan external-
    memory import (VK_EXT_external_memory_dma_buf + image_drm_format_modifier so
    the GPU samples the vendor-tiled buffer directly → tiling becomes structurally
    impossible AND the per-frame 3.7MB download disappears → unlocks 1080p@30 on
    FP5 Adreno 643/Turnip); force RHI Vulkan via QSG_RHI_BACKEND=vulkan; (D) do
    NV12→RGB + rotate + mirror + ROI overlay in the shader once on-GPU.
    DEFERRED (untestable here — no Qt/GL/Vulkan/device): all of B–D, plus wiring
    geometry::infer_stride into GstLaunchBackend's bloat path (would need a new
    videosrc C FFI seam + editing the load-bearing frame-read loop blind;
    declined to ship untested glue into the hot preview path — the inline
    heuristic already does equivalent inference, and the crate now serves as the
    verified reference + the layout math for the GPU import when it's built).
    HONEST: DMA-BUF format-modifier negotiation (libcamera→gst→Vulkan) can fail
    per-board → keep a Stage-B mapped-appsink CPU-upload fallback wired; Qt
    native-texture-import maturity varies by version/backend → needs on-device
    validation, not just API presence.

  - **GROUP DAG ↔ sealed-entry bridge — DONE + TESTED (next Rust-verifiable rung
    from the DAG work).** The DAG ordering layer now composes end-to-end with the
    confidentiality layer. DagNode gained to_payload()/from_payload(): a
    self-describing, round-trippable wire frame (magic "DAGN1", length-prefixed,
    SAME canonical sorted+deduped parent ordering as canonical_bytes) so a node
    serialized→shipped→parsed reconstructs with an IDENTICAL compute_id() — the
    content address survives the wire. GroupEntryBody gained from_dag_node()/
    to_dag_node(): wraps a node as a MediaRef body (payload = node.to_payload(),
    epoch from node, index = lamport as u32 hint) and recovers it; once sealed+
    signed the ordering metadata (parents/clock/content-hash) is itself
    encrypted+authenticated on the wire (DHT/relay see only opaque bytes).
    **45 group unit tests** (+5): payload_round_trips_and_preserves_id,
    from_payload_rejects_garbage, root_node_payload_round_trips,
    dag_node_rides_through_sealed_entry (THE end-to-end: author→wrap→seal+sign→
    verify+open→extract→insert into a fresh replica's DAG → admits with the same
    id + replicas agree on the tip), non_mediaref_body_is_not_a_dag_node. +1
    integration still green. LAYERING NOW PROVEN: DHT value → sealed+signed entry
    → DAG node, each tested, composed.
    NEXT Rust-verifiable rung after this: the sync loop primitives (announce
    tips, fetch missing_for deltas, request pending_fetches parents) as a tested
    state machine over the DAG + a transport trait the C++ implements; then
    sc_group_dag_* FFI if a non-Rust caller needs the bridge. Still DEFERRED (Qt/
    device): C++ group session + UI, snapclone-server replica.

  - **GROUP SYNC ENGINE — anti-entropy loop over the DAG. DONE + TESTED (the
    last big Rust-verifiable rung before the layer needs the device).** User said
    "just build it all" — declined to spray untestable Qt/Vulkan glue (a huge
    blind diff = more debugging, not less); instead built the next thing
    verifiable to the hilt: the loop that actually makes a group CONVERGE.
    NEW MODULE snapclone-group::sync — transport-free state machine. SyncMessage
    {AnnounceTips{tips} | WantNodes{ids} | HaveNode{node}}. SyncEngine wraps a
    SnapDag + an in_flight set (no duplicate WANTs): author() extends the DAG +
    returns an AnnounceTips; announce() = broadcast tips on a timer; handle(msg)
    runs the whole protocol — on_announce (want their tips we lack + push the
    missing_for delta as bounded HaveNodes), on_want (send requested nodes,
    bounded), on_have (insert; if orphan request missing parents, if admitted
    clear in-flight + request any still-pending parents; drop bad/dup). Does NO
    I/O — caller seals/signs each outgoing msg (a node = entry::from_dag_node →
    seal_and_sign) and ships it; inbound, caller verify+opens before handing the
    node here. SyncConfig bounds wants/haves per round. **52 group unit tests**
    (+7): two_peers_converge_to_same_total_order, divergent_histories_merge_both
    _ways (union both directions), out_of_order_delivery_still_converges (reversed
    HaveNodes → orphan-buffer + parent-request → complete), no_duplicate_wants_for
    _in_flight_nodes, already_synced_peers_are_quiet (≤2 rounds, no storm),
    author_returns_announce_with_new_tip, want_for_unknown_node_yields_nothing.
    +1 integration green. Release libsnapclone_group.a still builds (FFI/C++
    consumers unaffected — sync is internal pure-Rust). FULL GROUP STACK NOW
    RUST-PROVEN END TO END: ratchet/keys → sealed+signed entry → DAG ordering →
    DagNode↔entry bridge → anti-entropy sync convergence. Benign: one test-scope
    `use super::*` glob warning on DagNode (module import is load-bearing).
    REMAINING is genuinely device/Qt-bound (NOT skipped out of fear — skipped
    because unverifiable here): C++ group session driving SyncEngine over a real
    transport (DHT feed bucket + LAN + relay), group UI, snapclone-server passive
    replica, + an sc_group_sync_* / sc_group_dag_* FFI seam when the C++ caller
    is written. The decision engine — the hard, get-it-wrong-silently part — is
    done and tested; the glue is mechanical and best written WITH a compiler.

  - **VIDEO REPEAT-NEAR-BOTTOM + INTERMITTENT RED TINT (mobile) — root-caused +
    fixed. C++ wired, brace-checked, unverified on device.** USER: video still
    repeats near the bottom and is sometimes tinted red on mobile. Both are
    "bytes are right, layout interpretation wrong" bugs at the fdsink→QImage seam.
    REPEAT-NEAR-BOTTOM = STRIDE bug, now root-caused: the per-frame SLICE used
    m_frameBytes = w*h*4 (TIGHT) while the real pipe bytes are PADDED (each row
    rounded up to an alignment boundary). Slice too short → every frame starts a
    few bytes off → progressive diagonal drift → earlier rows reappear at the
    bottom. WORSE: the QImage was already built with a different stride guess, so
    it over-read past the slice. The code had learned about padded stride
    (m_sinkStride, "refined below") but NEVER actually refined it — m_sinkStride
    was always set to the tight w*4 and the slice never used it. FIX: (1) infer
    the real padded stride from buffer bloat (smallest alignment whose padded
    frame divides the buffer cleanly — same logic as the TESTED
    snapclone-videosrc::geometry::best_stride_guess), store as m_inferredStride;
    (2) the bloat path now ADOPTS that stride + re-aligns the buffer to a frame
    boundary instead of just logging+dropping; (3) ONE rowStride value now drives
    BOTH the per-frame slice (rowStride*frameH) AND the QImage bytesPerLine, so
    they can't disagree. m_inferredStride resets on each (re)start so a new
    negotiation re-infers. Ordering verified: bloat-check (sets stride, returns)
    runs before the slice loop, so no stale-arithmetic slice.
    RED TINT = R/B channel swap (BGRx read as RGBx). The auto-detect from sink
    caps was sound but only fires when caps carry a format string; "sometimes"
    tinted ⇒ the format isn't always reported on this libcamera build, OR it
    flips between negotiations. Couldn't disambiguate from here, so: (1) log the
    sink format on EVERY caps parse ("Sink format: X → swapRB B (override: F)")
    so the device log reveals whether it flips; (2) added m_swapRBForced so a
    user-pinned camera/policy/swapRB override is NEVER clobbered by auto-detect;
    (3) the manual override remains the reliable escape hatch. Brace-checked both
    files. VERIFY on device: repeat-near-bottom should be GONE (watch for
    "Adopting inferred row stride N" then a clean preview); for the tint, capture
    the "Sink format:" lines — if it flips, pin camera/policy/swapRB; if it's
    always BGRx yet still red, the bytes-on-wire disagree with caps (driver bug)
    and the manual swapRB=true is the fix.

  - **PREVIEW vs RECORDING split — confirmed the corruption is PREVIEW-ONLY +
    added a UI-boundary sanity guard. C++ wired + brace-checked; Rust guard
    TESTED.** USER hypothesis: "video seems fine but maybe the UI is the problem."
    INVESTIGATION (traced the full preview UI path, didn't guess): the live
    viewfinder path is onPreviewFrame → applyUserRotation (stride-safe;
    QImage::transformed makes a fresh packed image, returns src untouched at 0°)
    → optional mirrored() (fresh) → scaled(KeepAspectRatio) → setPixmap. EVERY
    step is stride-safe and the incoming QImage is a deep copy that owns its
    buffer (backend does .copy() before emit). CONCLUSION: the UI DISPLAY path is
    NOT introducing the tiling/repeat/tint — if they appear, the QImage was
    already malformed when the backend built it (the fdsink→QImage stride math,
    fixed last round). EVIDENCE the user's instinct (preview-specific) is RIGHT:
    recording is a SEPARATE gst→filesink pipeline that converts to I420 (YUV)
    before encoding — a colorspace with NO R/B channel concept, so the red tint
    is STRUCTURALLY IMPOSSIBLE in recordings; and recording uses GStreamer's
    stride-aware muxer, so the repeat-near-bottom (manual fdsink-byte slicing)
    also can't occur there. Both symptoms are confirmed preview-only, in the
    shared backend's frame construction — exactly where the stride fix went.
    ADDED (defensive, the genuinely useful thing vs a fake UI fix): a preview-
    frame sanity guard at the UI boundary (onPreviewFrame) — once per session /
    per device switch, checks the incoming QImage's bytesPerLine vs tight minimum
    and sizeInBytes vs stride×height, logging "Preview frame STRIDE TOO SMALL" /
    "TRUNCATED" / "BAD GEOMETRY" / "layout OK" so the next device run PROVES
    whether a malformed frame ever reaches the UI (turns "looks weird" into a
    precise line). Mirrors the TESTED snapclone-videosrc::validate_preview_frame
    (FrameValidity::{Ok|StrideTooSmall|SizeMismatch|BadGeometry}, **24 videosrc
    tests**, +4). m_previewFrameValidated flag resets on device switch.
    NET: stride fix (last round) targets the right layer; this guard confirms it
    on-device + the I420-vs-BGRx analysis explains why recordings were always
    clean. VERIFY: watch for "Preview frame layout OK" (fix worked) vs a STRIDE/
    TRUNCATED warning (residual case → the warning names the exact discrepancy).

  - **VIEWFINDER TINT — ROOT-CAUSE PARSER BUG FOUND. C++ fixed, brace-checked,
    parse logic proven; unverified on device.** USER pinned it: "playback is
    fine, the viewfinder page is messed up" — isolating corruption to the live
    preview (fdsink→QImage), NOT pipeline/wire/decoder. After several prior
    speculative preview fixes that didn't stick, stopped guessing and audited the
    DIAGNOSTIC that was supposed to catch the tint — and found why it never did.
    THE BUG: both caps-format parsers (source @~772 AND sink @~849) scanned from
    "format" to the first alphanumeric char to read the format token. But gst -v
    prints typed caps as `format=(string)BGRx` — so the scan stopped at the 's'
    in "(string)" and extracted "string" as the format. Proven: OLD parser on a
    real fdsink caps line returns 'string', NEW returns 'BGRx'. CONSEQUENCE: the
    R/B-swap auto-detect keys off sinkFormat.startsWith("RGB"); with the format
    always misread as "string", startsWith is ALWAYS false → the swap auto-detect
    could NEVER fire, no matter what the camera actually delivered. If the device
    delivers RGBx (or the GL glcolorconvert path swaps channels), the red tint was
    GUARANTEED and structurally undetectable — which explains why every prior
    preview fix missed it. FIX: both parsers now skip a parenthesised "(type)"
    tag (and '='/spaces) before reading the real token, so format reads correctly
    as BGRx/RGBx/NV12/etc. Now the existing auto-swap logic (and the every-parse
    "Sink format:" diagnostic added last round) will actually work. Brace-checked.
    HONEST: this fixes the DETECTION. Two outcomes possible on device now —
    (a) the format logs as RGBx → auto-swap engages → tint gone (the likely win);
    (b) it logs as BGRx yet still tinted → bytes-on-wire disagree with caps (GL
    path channel swap), and the manual camera/policy/swapRB=true is the lever,
    now reliable because m_swapRBForced (last round) protects it. Either way the
    "Sink format: X → swapRB B" line now shows the TRUTH instead of "string".
    NOTE on repeat/tiling: at 1280 (64-aligned) there's no stride padding per the
    device log, so the stride fix + inference handle the padded-width case; the
    sanity guard added last round will log if a malformed frame still reaches the
    UI. The tint was the parser bug; they were two separate causes under one
    "messed up" report.

  - **H.266/VVC (Fraunhofer) encode + decode — ADDED as opt-in backend. C++
    wired across 3 files, brace-checked, UNVERIFIED (needs a VVC-capable ffmpeg
    + device).** USER: "add fraunhoff h266 as an option to the encode and decode."
    DESIGN (honest about licensing + availability up front): VVC reference impls
    are Fraunhofer VVenC (enc)/VVdeC (dec); SOURCE is BSD-3-style (build/use
    fine) but VVC is PATENT-POOL covered (MPEG LA/Access Advance) → distribution
    is a project-owner business call, so this backend is OPT-IN and OFF by
    default, never silently baked in. ffmpeg drives it via libvvenc/libvvdec
    only if compiled in (most distro builds aren't). So: added as the LOWEST-
    priority backend, gated on BOTH SNAPCLONE_ENABLE_VVC=1 AND an availability
    probe — invisibly absent on a normal build, never displaces the working AV1
    path. ENCODE (VideoEncoderBackend.cpp): new FfmpegVvcBackend — name
    "ffmpeg-libvvenc", -c:v libvvenc, tier→preset (faster/fast/medium), CRF→-qp
    (constant-QP) or -b:v, yuv420p, ~1s force_key_frames on user tiers, aspect-
    preserving scale; ROI deliberately omitted (ffmpeg addroi side-data isn't
    consumed by libvvenc → would be a no-op). checkAvailable() probes `ffmpeg
    -encoders` for libvvenc/vvenc. CONTAINER: added IVideoEncoderBackend::
    outputContainer() virtual (default "webm"); VVC returns "mkv" (webm can't
    carry H.266). VideoTranscoder rewrites the output path's extension to match
    the selected backend's container (computed into correctedOutputPath since
    `spec` is const; applied to m_activeSpec + resolved + dir-create + exists-
    check + remove — all consistent). Registered last in
    allBackendsInPriorityOrder (reserve 5→6). DECODE: vvcDecodeAvailable() (probes
    `ffmpeg -decoders` for libvvdec/vvdec, same opt-in gate) declared in the
    header; VideoSnapViewerDialog now, when BOTH QMediaPlayer AND the gst fallback
    fail on a .mkv with no VVC decoder present, shows a SPECIFIC "this device
    can't decode H.266/VVC" overlay instead of the generic "backend unavailable"
    — turns an opaque black-video into an actionable message. (Playback itself
    works through the EXISTING Qt-ffmpeg / gst-decodebin paths when libvvdec is
    present — no hand-written decoder needed.) All 3 files brace-checked; includes
    verified (QFileInfo/QDir/QFile in transcoder; qEnvironmentVariableIsEmpty
    already used elsewhere in the backend file; viewer now includes
    VideoEncoderBackend.hpp). UNVERIFIED: no VVC-capable ffmpeg here to compile/
    run against. HONEST REMAINING for full end-to-end: the snap-SEND path may
    assume .webm somewhere (the recorder→transcode→send chain) — a received VVC
    snap arrives as .mkv, which the viewer handles, but verify the send/announce
    path doesn't hardcode the webm extension; tier→QP mapping is a reasonable
    guess, not perceptually tuned for VVC; VVC encode is HEAVY (likely too slow
    for real-time/standard tier on the FP5 — fine for experiments). To try:
    SNAPCLONE_ENABLE_VVC=1 with an ffmpeg built --enable-libvvenc --enable-libvvdec.

  - **VVC integration follow-through — closed the two send/receive container gaps
    I flagged. C++ wired (3 files), brace-checked, unverified.** "keep working" →
    instead of new surface, audited the gaps I'd FLAGGED in the VVC round (those
    are grep-checkable here, no compiler needed). Found and resolved both:
    (1) SEND path (CameraWidget): onTranscodeFinished is wired straight to
    transcodeFinished, which emits the ACTUAL output path — so a VVC .mkv is sent
    correctly and m_recordingPath (the original raw recording) is what's removed.
    The logic was already right; only the param/local NAME ("webmPath") and a
    stale "remove the .mkv, send the .webm" comment were misleading. Renamed
    onTranscodeFinished(outputPath) + the requestedOut local + corrected comments
    so a future reader doesn't "fix" it wrongly. Header decl updated to match.
    (2) RECEIVE path (SnapRouter::detectMediaExtension) — THIS was genuinely
    broken: a received VVC file is matroska with EBML magic IDENTICAL to webm, so
    it was written as ".webm". But webm by spec can't carry H.266, so external
    players reject it AND the viewer's VVC-decode-failure hint (added last round)
    keys on ".mkv" → a received VVC snap would land as .webm, miss the hint, and
    fail opaquely. FIX: when EBML magic matches, scan a bounded 64KiB prefix for
    the matroska CodecID "V_MPEGH/ISO/VVC" (and HEVC, also non-webm) or an
    explicit "matroska" DocType preceding "webm" → return ".mkv"; else ".webm"
    as before. Used qMin<qsizetype> (no <algorithm> include needed). Brace-checked
    all three. NET: the VVC path is now consistent end to end — encode→.mkv,
    send→real path, receive→detected as .mkv→viewer can attempt VVC decode / show
    the specific hint. Still unverified (no VVC ffmpeg here); still gated on
    SNAPCLONE_ENABLE_VVC. The EBML/CodecID sniff is a good candidate to extract
    into a tested helper later (pure byte logic).

  - **MAILBOX/GROUP/CONTACT-EDIT dynamic submenus — built the tested MODEL +
    FFI; Qt wiring DEFERRED.** USER: "keep working on mailbox ui, group creation,
    group mailbox ui, edit contact as a submenu… some or all dynamic submenus."
    INVESTIGATION: the contact card (ContactCard) ALREADY has the dynamic-submenu
    mechanism — an animated collapsible m_submenu + addAction(label, cb, icon,
    danger); ContactsPage::buildCard already populates it CONDITIONALLY (Edit /
    Share-if-shareable / Copy / Remove-if-removable, lines ~761-790). So this
    request is: extend that pattern to groups + mailbox rows, turn Edit into an
    inline-expand option, and centralize the now-multiplying "which actions show"
    branching. The decision logic (entity state → ordered action list) is pure
    data→list logic = the verifiable rung; the rest is Qt widget glue I can't
    compile here.
    SHIPPED + TESTED: NEW crate snapclone-menu (ZERO deps, staticlib+rlib).
    actions_for(Entity{kind,state}, edit_inline) → ordered Vec<MenuAction{id,
    kind,danger}> for all 4 surfaces: Contact (edit/share/copy/unblock/remove),
    Group (open/rename/manage-members[admin]/mute/leave), MailboxEntry (pending →
    accept/reject/block; active → open/mute/remove), + header_actions(Group) →
    create_group. ActionId carries stable tag()/default_label()/icon();
    ActionKind {Immediate|Dialog|Expand} expresses edit-as-dialog vs edit-as-
    inline-panel (edit_inline flips Edit/Rename to Expand). **12 model tests**
    incl editable_removable_contact_matches_legacy_order (PROVES adopting the
    model is behavior-preserving for the current card), admin vs member group
    action sets, pending vs active mailbox, blocked→unblock, muted→unmute,
    danger-flag placement, unique tags. C FFI (ffi.rs): sc_menu_actions_for(kind,
    state_bits bitfield, edit_inline, ScMenuAction* out, cap)->count +
    sc_menu_header_actions; caller-owned buffer, 'static strings (no free),
    panic-safe + null/bad-kind tolerant, capacity-respecting. **5 FFI tests.**
    Header include/menu/snapclone_menu_ffi.h passes gcc -fsyntax-only C AND C++.
    README + workspace member added. Tested STANDALONE (17/17) + release
    libsnapclone_menu.a builds (zero deps; in-workspace cargo hits the known
    getrandom/edition2024 drift from siblings → build -p individually).
    DEFERRED (Qt, untestable here): wire ContactsPage::buildCard to drive
    actions_for instead of inline ifs (behavior-preserving per the pinned test);
    build the GROUP card + group-creation dialog + group mailbox rows reusing
    ContactCard's submenu mechanism + the model; implement the inline-edit Expand
    panel (vs the existing ContactEditorDialog). The hard part (consistent action
    policy across surfaces) is done + tested; the glue is mechanical + best
    written with a compiler. Group data/session UI also still needs the C++ group
    session over the tested snapclone-group crate (separately deferred).

  - **Media container sniffing — extracted to a TESTED Rust module (closes the
    flagged gap).** "keep working" → took the gap I'd flagged last round (the
    EBML/codec-ID sniff in SnapRouter::detectMediaExtension is pure byte logic,
    was UNTESTED) — same play as the stride-geometry extraction. NEW module
    snapclone-videosrc::sniff_container(bytes, MediaHint)->MediaContainer{WebM|
    Matroska|Mp4|Mov|Jpeg|Png|UnknownVideo|UnknownOther} + .extension() +
    sniff_extension(). Mirrors the C++ EXACTLY incl the VVC/HEVC-in-matroska
    distinction (the actual bug from the VVC round). **10 tests**: webm vs
    matroska doctype, vvc_codec_forces_mkv_even_without_doctype (the regression
    that mislabeled VVC as .webm), hevc→mkv, plain-ebml→webm, mp4 ftyp, mov
    moov/mdat, jpeg+png, unknown→hint fallback, short_buffers_dont_panic.
    **34 videosrc tests total.** Left a sync-note in SnapRouter.cpp naming the
    tested module as the authority so the C++ copy + Rust don't silently drift
    (or drive it via FFI if it grows). SnapRouter.cpp brace-checked. This decides
    the file extension for EVERY received snap (→ whether it plays), so a tested
    reference is real insurance. DEFERRED: an FFI seam + actually calling it from
    C++ (the inline copy works; the crate is the verified reference for now).

  - **★ BUILD TOOLING INSTALLED — C++ now COMPILE-CHECKABLE here (major workflow
    upgrade).** USER asked to install cmake + rust/cargo to actually test outputs.
    Result: cmake 3.28.3 installed via apt (Ubuntu mirrors are allowlisted; had
    to remove the blocked nodesource repo first). Qt 6.4.2 dev stack installed:
    qt6-base-dev + qt6-multimedia-dev + qt6-svg-dev (covers Core/Gui/Widgets/
    Multimedia/MultimediaWidgets/Svg/Network/Sql/Concurrent — all the modules the
    code uses). cargo/rustc 1.75 + gcc 13.3 were already present.
    SYNTAX-CHECK RECIPE (sh-safe; note PROJECT IS C++20, not 17):
      QTINC=/usr/include/x86_64-linux-gnu/qt6
      g++ -fsyntax-only -std=c++20 -fPIC -DSNAPCLONE_RUST_SALIENCY \
        -I$QTINC -I$QTINC/QtCore -I$QTINC/QtGui -I$QTINC/QtWidgets \
        -I$QTINC/QtMultimedia -I$QTINC/QtMultimediaWidgets -I$QTINC/QtSvg \
        -I$QTINC/QtNetwork -I$QTINC/QtSql -I$QTINC/QtConcurrent \
        -I include -I src <file.cpp>
    (script saved at /tmp/qtsyntax.sh during this session.)
    ALL 6 C++ FILES EDITED THIS SESSION COMPILE CLEAN under real Qt6 + C++20
    (not just brace-checked): SnapRouter.cpp (container/VVC sniff),
    VideoEncoderBackend.cpp (VVC backend + outputContainer virtual),
    VideoTranscoder.cpp (container rewrite), GstLaunchBackend.cpp (caps-parser
    (string) fix + stride inference), CameraWidget.cpp (preview sanity guard),
    VideoSnapViewerDialog.cpp (VVC decode-fail message). Found+fixed nothing
    further — they were correct, now PROVEN to parse against real headers.
    CAVEAT: Qt 6.4.2 here is OLDER than the device's Qt6; APIs I specced for the
    GPU camera path (QRhiTexture::createFrom, 6.6+) aren't testable here and a
    full cmake configure still needs OpenDHT/GStreamer/PipeWire dev libs (not
    installed). So this is SYNTAX/TYPE checking of individual TUs, not a full
    link of the app — but it catches the class of error brace-checking can't
    (wrong std, bad signatures, missing/renamed symbols, type mismatches).
    RUST: the new zero-dep crates now test IN-WORKSPACE (no more temp-copy):
    cargo test -p snapclone-menu (17), -p snapclone-videosrc (34), -p
    snapclone-group (52+1), -p snapclone-gesture (15) all green. Full-workspace
    `cargo build` STILL hits the getrandom/edition2024 drift (cargo 1.75 too old;
    rustup blocked by network allowlist) — confined to crates pulling that
    transitive dep; the zero-dep crates resolve fine individually.
    ★ STRONGEST CHECK YET: linked a C++ consumer against the real
    libsnapclone_menu.a and RAN it — correct output (contact: edit/share/copy/
    remove[danger]; group header: create_group). Proves header ABI + symbol names
    + logic line up across the FFI boundary, end to end, not just in theory.

  - **CAMERA UX bug report (7 items) — 3 fixed+compiled, 4 need device logs.**
    USER report: (1) double-tap to flip front/rear; (2) "mouse click shows AF box
    but not tapping"; (3) cameras always tiled on latest build, once cleared red
    by switching cameras; (4) videos sometimes don't preroll when recording; (5)
    front/rear sometimes mixed up; (6) sometimes got colors right but never
    tiling; (7) preview should be full-screen behind the buttons in ambient mode.
    FIXED + COMPILE-VERIFIED (real Qt6/C++20):
      * Bugs 1+2 (CameraWidget.cpp): the viewfinder tap path was MOUSE-ONLY
        (eventFilter handled only MouseButtonPress; widget never set
        WA_AcceptTouchEvents) → on Plasma Mobile finger taps arrived as
        QTouchEvent that Qt didn't synthesize to mouse → "mouse works, tap
        doesn't". Now: set WA_AcceptTouchEvents on the viewfinder + handle
        TouchBegin, funnel both into new handleViewfinderTap(point). That handler
        implements single-tap=focus / double-tap=flip: a tap arms a single-shot
        m_pendingFocusTimer (kDoubleTapMs=280); a 2nd tap inside the window
        cancels focus and calls flipCameraFacing(); <40ms = digitizer bounce
        (ignored). flipCameraFacing() finds the next combo entry with the
        opposite Facing (via CameraRegistry::instance().byId) and setCurrentIndex
        → fires onSelectorChanged, the SAME path the menu uses (no parallel
        switch logic). NOTE the old 250ms ghost-debounce that SWALLOWED 2nd taps
        is gone (it'd have eaten the flip). Tradeoff: tap-focus now delayed
        ~280ms (conventional phone-camera behavior). [CAUGHT BY COMPILER: first
        draft used a nonexistent CameraRegistry::lookup()→ptr; real API is
        byId()→optional. Brace-check would've missed this.]
      * Bug 3/6 escape hatch (GstLaunchBackend.cpp): added SNAPCLONE_CAMERA_STRIDE
        env override (bytes/row) — highest priority over inference; when set,
        stride inference is skipped so it can't fight the pinned value. Lets you
        confirm-and-discover the true stride on-device the way
        SNAPCLONE_CAMERA_ROTATION debugs rotation. (Color already had
        camera/policy/swapRB; tiling had no manual pin until now.)
    STILL NEED DEVICE LOGS (can't fix blind):
      * Bug 3/6 tiling persists across reloads → inference is picking the wrong
        stride (or none) for the FP5's real buffer geometry. Need: the
        "Frame buffer bloat… Stride candidates:" + "Adopting inferred row
        stride" lines (or their absence), and the Negotiated SINK caps line.
        Then SNAPCLONE_CAMERA_STRIDE to find the right value. KEY CLUE from user:
        color (R/B swap) and tiling (stride) are INDEPENDENT — switching cameras
        cleared red but never tiling.
      * Bug 4 preroll-on-record → the gst record pipeline isn't reaching PLAYING.
        Need the recorder/transcoder log around a failed record start.
      * Bug 5 front/rear mixup → likely CameraRegistry node→facing mapping on
        FP5 (the i2c-bus heuristic). Need: which device label was selected vs
        which sensor actually appeared, + the node names from pw-dump.
    DEFERRED (verifiable but needs a screen, not a compiler):
      * Bug 7 full-screen ambient preview: current layout is topBar/viewfinder
        (stretch)/bottomBar in a QVBoxLayout. Full-bleed-behind-buttons means
        making the viewfinder fill the widget and re-parenting the bars as
        floating overlay children (the reticle/AF-badge/record-indicator ALREADY
        do this as viewfinder children — same pattern), ideally gated to ambient
        mode. Bounded change but correctness is VISUAL — won't ship a blind
        layout rewrite into the main camera path; best done where it renders.

  - **CAMERA root-cause from device log: negotiation-fail → silent camera SWAP
    was Bugs 4+5 (+ the "switch fixes it" mystery). FIXED + compiled.** The FP5
    log shows the SAME pattern across every capture: rear node ...camera_29 hits
    "Format negotiation failed / not-negotiated (-4)" intermittently, and the
    backend's ONLY recovery was to jump to the OTHER camera (...camera_10, front)
    with the SAME forced caps. That IS the front/rear mixup (Bug 5): a transient
    rear-negotiation hiccup silently lands you on the front cam. It's also why
    "switching cameras fixed colors/tiling once" — the swap changed which
    physical sensor (and buffer geometry) was live. Evidence it's transient not a
    hw limit: ...camera_29 streams 1280x720 bytes-per-frame=3686400 fine in the
    same logs moments later. ROOT CAUSE: the GPU pipeline pins
    `format=BGRx,width=1280,height=720` on the GLMemory caps (and videoconvert +
    CPU-path caps) even though the SOURCE caps are deliberately left free — so
    libcamera loses the size negotiation right after a stop/start before the
    prior stream releases. FIX (GstLaunchBackend.cpp): on a NEGOTIATION-specific
    failure (not-negotiated / Could not negotiate / Format negotiation failed),
    the FIRST recovery is now to relaunch the SAME node with RELAXED caps (drop
    the width/height pins; keep format) via new m_relaxCaps, gated by
    m_retriedRelaxed (one relaxed retry/session) + m_lastRelaxedNode (so our own
    retry start() keeps relax while a fresh user start clears it). Only if the
    relaxed same-node retry ALSO fails do we fall to the alternate camera. Non-
    negotiation failures (missing node, busy) keep the old swap behavior. All
    three caps sites now branch on m_relaxCaps. NOTE: whether dropping w/h
    actually lets libcamera negotiate on FP5 is the DEVICE-bound hypothesis — the
    retry LADDER + relaxed caps construction are compile-verified; the on-device
    outcome needs a log. Tradeoff: a relaxed stream may tile (no size pin) but a
    tiling stream beats one that won't start; the stride inference still runs.
  - **Focus: analyze-before-move + interruptible sweep (user's insight). FIXED +
    compiled.** USER: "separate the camera adjustment from the analysis… if our
    image is already sharp we don't need to fire… when the double tap comes in we
    can exit early and dump the cheap cast." Implemented both halves:
    (1) SKIP-IF-SHARP (CameraWidget::runFastFocusSweep): before driving the VCM,
    measure the current frame's laplacianSharpness (already a standalone fn in
    FocusSweep.hpp) and if it's >= 90% of m_focusBaseline (the sharpness from the
    last GOOD focus — the SAME signal scene-change re-focus trusts, NOT a new
    magic absolute threshold), confirm the reticle Locked and skip the motor
    sweep entirely.
    (2) INTERRUPTIBLE SWEEP: added an optional `std::function<bool()> isCancelled`
    param to runFastFocus (defaulted, so no other caller changes — verified only
    one call site). The worker polls it before EACH lens move + before the final
    park, returning succeeded=false without moving further if set. New atomic
    m_focusAbort is cleared when a sweep is armed and SET by the double-tap-flip
    branch in handleViewfinderTap — so a flip stops the lens mid-sweep ("dump the
    cheap cast") instead of completing it. Atomic across the GUI/worker boundary.
    DEFERRED (Qt/device, not built): the crossfade-from-last-known-frame on
    camera switch (user's "pull up a last-known image and crossfade so switching
    is faster") — that's viewfinder compositing best done where it renders; the
    backend relaxed-retry above already makes the switch itself more RELIABLE,
    which was the worse problem.

  - **"Video snaps not coming through" — AUDITED full send/recv/display chain;
    found NO regression. Added a decisive receive-stage diagnostic.** USER asked
    to check the FSM manager + basics for a break in snap send/receive + whether
    the new UI populates received snaps. Traced end to end:
      * No class literally named "FsmManager"; the snap path is SnapRouter →
        AppController → MainWindow. inboundReady (SnapRouter.cpp 901/1211/1347) →
        AppController_connect.cpp:581 lambda emits snapReceived → MainWindow.cpp:
        496 handler calls m_inboxPage->addSnap(snap) for EVERY snap, no
        video/photo discrimination. Wiring intact.
      * Re-audited my recent detectMediaExtension edit (SnapRouter): it only
        picks the FILE EXTENSION of a received video; it cannot drop reception.
        Matches the tested Rust sniff_container reference. Not the cause.
      * InboxWidget::addSnap stores every snap in m_snaps + rebuilds; it dedups
        the BADGE COUNT only, never rejects. Viewer routing (InboxWidget ~1061)
        keys on snap.hasVideo() == (mediaType==Video), NOT on file extension, so
        the extension work can't misroute the viewer either.
      CONCLUSION: the send/receive/display code is structurally sound — no
      manufactured fix. The laptop log CANNOT show a video "coming through"
      because both peers were OFFLINE (every line after "sent" is
      [RECONNECT] ... disconnected): the snap was QUEUED for offline peers, and
      there's no 2nd online device in that log to confirm receipt. Same
      single-device-log limitation as before.
      SHIPPED: a diagnostic at THE UI population point (InboxWidget::addSnap) —
      logs id/sender/mediaType/(VIDEO|image)/path/alreadyKnown at qCInfo
      (snapclone.inbox). On the phones this pinpoints the failure STAGE: line
      ABSENT for a sent snap ⇒ never reached UI (delivery/router/decrypt upstream
      — look at inboundReady + the v3 DATA/ANNOUNCE path on the RECEIVER);
      line PRESENT ⇒ arrived, issue is downstream (rebuild/render/viewer).
      NEED FROM PHONES: a RECEIVER-side log (not sender) filtered on
      snapclone.inbox + snapclone.router around a video send between two ONLINE
      phones — that's the only thing that can actually show this bug.

  - **★ ROOT CAUSE of "snaps not coming through": LAN-direct never connects two
    same-WiFi phones — THREE coordinated fixes, all compile-verified + the
    ranking RAN against the real FP5 scenario.** Device log proved it: phone
    announces `ws://172.16.42.1:9876` — that's usb0 (USB tether, 172.16.42.1/16
    from `ip a`), NOT wlan0 (192.168.2.223). Both phones on one router, but each
    advertises a USB-net IP the other can't reach over WiFi → DirectDiscovery's
    subnet check skips the LAN probe ("not on our subnet") → no session → every
    snap logs "No active session — queuing" + "pending ACK at seq 1", never
    delivered. Public NAT path also dead (STUN DNS fails, UPnP+NAT-PMP fail).
    THREE bugs, all fixed:
    (1) ANNOUNCE side (GossipDiscovery::announcePresence): was advertising the
        FIRST non-loopback IPv4 from allAddresses() — usb0 enumerates before
        wlan0 on FP5. NEW tested helper AddressClass::pickBestLanAddress() /
        lanAdvertiseRank() in AddressClassification.hpp ranks 192.168/16 (100) >
        10/8 (80) > 172.16/12 (40, incl the tether) > link-local (10) > else (0).
        announcePresence now picks the best-ranked candidate. ★ COMPILED A TEST
        HARNESS AND RAN IT against the exact log scenario {172.16.42.1 first,
        192.168.2.223 second} → picks 192.168.2.223; all rank assertions pass.
    (2) RECEIVE side (GossipDiscovery::processDatagram): was trusting the
        payload's self-reported host. Now, if the datagram SOURCE IP is private
        (AddressClass::isPrivate), prefer it — it's provably reachable on this
        subnet. Corrects a wrong announce on arrival (defense in depth with #1).
    (3) WIRING (the dead link): DirectDiscovery::updateKnownPeer existed but was
        NEVER CALLED, and gossip's discovered addresses never reached it — so the
        LAN probe only ever saw storage-loaded PUBLIC addresses. Added
        NetworkManager::lanPeerAddressLearned signal (emitted in onPeerDiscovered
        BEFORE the hairpin/rate-limit returns, so even a rate-limited address is
        still remembered as a LAN candidate), wired in AppController to
        directDiscovery->updateKnownPeer. Now the gossip-learned 192.168.x addr
        actually populates the probe table.
    Installed qt6-websockets-dev to compile-check AppController (pulls
    RelayTransport→QWebSocket). All 4 files compile under real Qt6+C++20 with
    -DSNAPCLONE_OPENDHT_BACKEND. NOTE: needs the two-online-phones test to
    confirm end-to-end; this is the strongest theory yet and the announce-side
    ranking is execution-verified, but the on-device session handshake after the
    LAN probe fires is the part only a device can confirm. Watch for: gossip
    "picked LAN addr 192.168.x", DirectDiscovery "LAN probe: trying ... @
    192.168.x" (instead of "not on our subnet"), then a session + ACK.

  - **Unified-mailbox dynamic submenus: built the MISSING DATA LAYER (tested),
    not blind Qt.** USER: "keep working on the new dynamic submenus for unified
    mailbox and group chats." INVESTIGATION first: the snapclone-menu MODEL + FFI
    exist + are tested but still NOT wired into Qt (grep confirms zero C++ uses).
    "Unified mailbox" is the InboxWidget under unified-nav mode, NOT a separate
    widget. Tried to wire the menu model into InboxWidget rows — but hit the real
    blocker: ConversationRow carries ONLY conversation summary (otherParty,
    lastSnapAt, unreadCount, thumbnail) — NO pending/muted/blocked/group state.
    And there's no persistent connection-request approval list anywhere:
    connectionRequestReceived flows to an approval path but nothing models the
    pending→accepted/rejected/blocked lifecycle a unified mailbox would render as
    accept/reject/block rows. The existing snapclone-mailbox crate models the DHT
    TRANSFER rendezvous (delivery-pending), not the UI APPROVAL lifecycle —
    different concern. So wiring the menu model now would mean fabricating state
    or hardcoding every entry "active" (making the dynamic model pointless).
    DECISION: build the missing data foundation as tested logic instead of
    spraying Qt against absent state.
    SHIPPED + TESTED: snapclone-mailbox::requests (NEW module). RequestMailbox
    state machine: receive(sender,name,ms)->bool admits a request as Pending
    (drops if sender blocked; re-opens a prior Rejected/Accepted on re-request;
    refreshes-not-stacks a duplicate Pending); apply(sender, Accept|Reject|Block)
    ->ApplyOutcome{Changed(state)|NoChange|Refused} enforces the invariants
    (Accept-on-Blocked REFUSED; Block-after-Accept allowed; re-accept idempotent;
    action on unknown id = no-op); unblock(); pending() newest-first for the
    mailbox rows; pending_count() for the badge; state_of() for the menu model to
    read. **11 new tests** (22 total in crate, all green in-workspace) covering
    every transition incl block-drops-future-requests, accept-on-blocked-refused,
    reject-allows-re-request, duplicate-doesn't-stack, sorted-newest-first.
    ALIGNMENT VERIFIED: menu model's EntityState.pending ↔ RequestState::Pending,
    and AcceptRequest/RejectRequest/BlockSender ↔ RequestAction::Accept/Reject/
    Block — the data layer (what state) and presentation layer (what actions that
    state shows) compose by design. Both halves are now tested logic.
    DEFERRED (Qt glue, now UNBLOCKED + compile-checkable here): FFI seam for
    RequestMailbox; populate InboxWidget pending-request rows from it; drive each
    row's submenu via sc_menu_actions_for(MAILBOX, pending-bit); wire
    Accept/Reject/Block buttons to apply(); the group-chat half still also needs
    the C++ group session over snapclone-group (separately deferred). Next rung:
    the RequestMailbox FFI + an InboxWidget pending-rows section, both now
    verifiable with the compiler.

  - **RequestMailbox C FFI — built, tested, and PROVEN end-to-end (link+run).**
    The seam that unblocks the unified-mailbox Qt wiring. NEW
    rust/snapclone-mailbox/src/requests_ffi.rs: opaque ScRequestMailbox* handle,
    panic-safe (catch_unwind), null-tolerant. Functions: sc_reqmb_new/free,
    sc_reqmb_receive(mb,sender,name,ms)->1|0, sc_reqmb_apply(mb,sender,action)
    ->encoded ApplyOutcome (NOCHANGE=0, REFUSED=-1, CHANGED_BASE+state=101..103),
    sc_reqmb_unblock, sc_reqmb_state_of, sc_reqmb_pending_count,
    sc_reqmb_pending(mb,ScPendingRow*,cap)->n (caller-owned char buffers per row,
    NUL-terminated + truncation-safe). Consts SC_REQ_STATE_*/ACTION_*/OUTCOME_*.
    Added "staticlib" to crate-type (mailbox crate builds clean here — its
    serde/sha2/chrono deps don't hit the workspace edition2024 drift; they link
    in unused by the FFI surface, fine). 5 FFI tests (lifecycle_roundtrip,
    block_then_accept_refused, pending_rows_filled, null_tolerant,
    buffer_truncation_safe) → 27 total in crate, all green.
    ★ STRONGEST VERIFICATION (same as the menu FFI): wrote C header
    include/mailbox/snapclone_requests_ffi.h, validated it compiles as BOTH C11
    AND C++20, built the release libsnapclone_mailbox.a (60MB), and compiled +
    LINKED + RAN a real C++ consumer (/tmp/reqmb_consumer.cpp) against it: two
    requests render newest-first, Accept removes from pending, Block drops the
    re-request + refuses subsequent Accept, state_of returns correct states. ALL
    C++ CONSUMER ASSERTIONS PASSED. The encoded-outcome scheme works across the
    ABI (Accept→101 = CHANGED_BASE+ACCEPTED observed at runtime).
    NOW UNBLOCKED (next rung, compile-checkable in Qt): InboxWidget owns a
    ScRequestMailbox*; call sc_reqmb_receive on connectionRequestReceived; add a
    pending-requests section that renders sc_reqmb_pending rows; drive each row's
    submenu via the snapclone-menu actions (pending-bit → accept/reject/block);
    wire those buttons to sc_reqmb_apply; badge from sc_reqmb_pending_count. The
    menu FFI (sc_menu_actions_for) + this request FFI now compose into the
    dynamic submenu with zero fabricated state.

  - **★ Unified-mailbox dynamic submenus — WIRED INTO QT (compile-verified) +
    the two-FFI composition proven at runtime.** The Qt half that last turn's
    FFI seam unblocked. Both tested Rust models now drive real InboxWidget UI
    with ZERO fabricated state:
    HEADER (include/ui/InboxWidget.hpp): global fwd-decl `struct ScRequestMailbox`
    (opaque FFI handle, no C header pulled into the C++ header); fwd-decl
    ContactCard (reused as the row widget); new members m_pendingSection/
    m_pendingLayout/m_pendingHeader + m_requestMailbox handle; public
    receiveConnectionRequest(senderId,name) + pendingRequestCount(); signals
    connectionRequestAccepted/Rejected + senderBlocked (for AppController to act
    on); added ~InboxWidget() override.
    IMPL (src/ui/InboxWidget.cpp): buildPendingRequestsSection mounts ABOVE the
    outbound banner (requests are top-priority); creates the FFI model via
    sc_reqmb_new (freed in dtor via null-tolerant sc_reqmb_free).
    receiveConnectionRequest → sc_reqmb_receive + rebuild. rebuildPendingRequests
    pulls sc_reqmb_pending into caller-owned buffers, fetches the PENDING-state
    actions ONCE via sc_menu_actions_for(SC_MENU_KIND_MAILBOX,
    SC_MENU_STATE_PENDING), and renders each request as a ContactCard whose
    expand-submenu actions are populated from the menu model and dispatched by
    stable tag (accept_request/reject_request/block_sender) to applyRequestAction
    → sc_reqmb_apply. applyRequestAction only emits the host signal when the
    model actually transitioned (outcome >= CHANGED_BASE), so a blocked-then-
    accept (REFUSED) correctly fires nothing.
    ★ RUNTIME-VERIFIED the integration assumption: built libsnapclone_menu.a and
    ran a C++ probe — sc_menu_actions_for(MAILBOX, PENDING) returns EXACTLY the 3
    tags the Qt dispatcher branches on (accept_request/reject_request/
    block_sender), with block_sender danger=1. So the menu→request dispatch
    matches with no silent skips. InboxWidget.cpp + its header both compile under
    real Qt6 + C++20.
    Fixes found while wiring: FONT_MD doesn't exist → FONT_BASE; the opaque
    handle needed a global-scope (not namespaced) fwd-decl to match C linkage.
    NEXT SEAM (deliberately deferred — touches the connection-approval machinery,
    wants its own focused pass): (a) AppController calls
    inbox->receiveConnectionRequest when DhtDiscovery::connectionRequestReceived
    fires + decrypts to a pending approval (the decrypt path is at
    AppController.cpp ~243); (b) AppController consumes
    connectionRequestAccepted/Rejected/senderBlocked to actually establish the
    connection / persist a blocklist entry. Wire in MainWindow alongside the
    existing m_inboxPage connects (~line 267). UNVERIFIED on device (UI is
    compile-checked only; no widget render test possible here).

  - **★ CAMERA AUDIT (color/tiling, fresh pass) — found + fixed a REAL stride
    bug: C++ inference had drifted from the tested Rust authority.** USER asked
    to re-audit cameras for color/tiling bugs in paths not looked at recently.
    Anchored to this session's device log GROUND TRUTH first: live preview is
    1280x720 BGRx, bytes-per-frame=3686400 (=1280*720*4 exactly, NO padding),
    bpl=5120 (=1280*4, tight), "Preview frame layout OK ... UI display path is
    clean". So the CURRENT device case has no tiling — but the audit asked
    whether OTHER resolutions could.
    AUDIT FINDINGS:
      * Preview QImage construction (GstLaunchBackend ~1505): correct — imgStride
        = the same rowStride the slice was cut at, so bytesPerLine matches the
        buffer. Sound.
      * Face-detect color path (CameraWidget ~1383): correct — copies via
        constScanLine(y), doesn't assume tight stride. Sound.
      * Recorder rotation (VideoRecorder ~440 videoflip + transcoder auto-rotate):
        device log shows 720x1280 storage + '-90° Display Matrix' → transcode
        auto-rotates → coherent 16:9 output. Device-verified working in a prior
        session; did NOT touch it (would be speculative). Noted latent fragility:
        depends on ffmpeg's version-dependent default auto-rotate.
      * ★ THE REAL BUG — stride inference drift: snapclone-videosrc::infer_stride
        / best_stride_guess (TESTED, 34 tests) is the extracted authority and
        uses EXACT division (observed_total % frame_size == 0). The inline C++
        copy in GstLaunchBackend::onStdoutData had DRIFTED to a FUZZY test
        (`m_buf.size() % padFrame < padFrame/8`, ~12.5% tolerance). Demonstrated
        with a Rust harness: for a 1284-wide (align-64-padded) buffer, the fuzzy
        test FALSELY also accepts align=32 (remainder 46080 < tolerance 463680),
        and since the loop takes the smallest matching alignment, it would adopt
        the WRONG stride (5136 vs 5184) → causes the exact diagonal tiling it
        meant to fix. FIXED: changed C++ to exact division `== 0` to match the
        tested Rust. The device 1280x720 case yields "no padding" under both
        (tight divides cleanly) so the working path is undisturbed; the fix only
        affects non-aligned widths. Also corrected the misleading
        `m_sinkStride = sinkW*4 // refined below` comment (it's never refined; gst
        -v caps don't expose row padding — only the bloat inference catches it).
      FOLLOW-UP (noted, not done): drive the C++ inference from
        snapclone-videosrc via FFI so the two can't drift again — same treatment
        as menu/request models. The geometry fns (round_up/padded_stride/
        frame_size/infer_stride/best_stride_guess) are FFI-ready (zero-dep, pure).
      GstLaunchBackend.cpp compiles under real Qt6+C++20. Stride math verified by
      executing a Rust harness against device-log dims. On-device effect only
      observable on a non-64-aligned capture resolution (1280 is aligned, so this
      won't change the current FP5 preview — it removes a latent trap).

  - **★ SYSTEMATIC CAMERA AUDIT (wide, category-based sweep) — found + fixed a
    real GPU-path latch bug; mapped coverage so future audits don't re-tread.**
    USER asked for a wider, systematic way to check ALL camera pathways. METHOD:
    enumerated the full surface (38 files, ~9830 lines; CameraWidget 3028 +
    GstLaunchBackend 1715 are the complexity centers), then swept 8 BUG
    CATEGORIES with targeted greps across the whole surface at once rather than
    reading file-by-file:
      1. Integer overflow in size math (w*h*bpp): m_frameBytes is int, but 4K
         (33M) << int32 max — safe at camera resolutions. CLEAN.
      2. Truncating size casts: only the defensive bpl validation cast. CLEAN.
      3. ★ Stale state flags on start() — diffed ALL state members against those
         reset in start(). Found m_gpuConvertFailed is set true on a GL-link
         failure (line 1054) but NEVER reset anywhere. A single TRANSIENT GPU
         failure (GL ctx not ready during a fast switch, driver hiccup) forced
         the slower CPU colorspace path for the WHOLE app lifetime across every
         subsequent start (FP5's normal path IS the GPU one). FIXED: reset
         m_gpuConvertFailed=false on NODE CHANGE (alongside m_qualityRestartUsed)
         — fresh GPU attempt per user session, but NOT during the same-node 100ms
         recovery retry (node==m_currentNode there, so no retry loop). Compiles.
      4. QImage format / R-B byte order: GstLaunchBackend swapRB auto-detect +
         override sound; QtCameraBackend format guard sound. CLEAN.
      5. Rotation consistency (preview/photo/video): videoflip + transcoder
         auto-rotate produce coherent output (device-log verified 16:9). Latent
         ffmpeg-version-dependence noted, untouched (working). CLEAN.
      6. Buffer-bounds on scanLine/bits/QImage-from-raw: GstLaunchBackend QImage
         invariant verified — sliceBytes = rowStride*frameH == frame.size(),
         imgStride*imgH == sliceBytes, QImage reads exactly in-bounds. The
         face-detect + Qt YUV paths copy via scanLine, no overrun. CLEAN.
      7. VCM/focus numerics: BT.601 limited-range YUV→RGB coeffs (298/409/100/
         208/516, Y-16, UV-128) + clamps CORRECT; I420/NV12/NV21 U/V plane
         handling correct. (Minor: QtCameraBackend uses BT.601 while device
         records BT.709 — a small saturation shift on the laptop preview path
         only, not a bug.) CLEAN.
      8. Cross-thread frame handoff: GstLaunchBackend m_lastFrame written+read on
         the GUI thread (QProcess signals); no unguarded cross-thread race.
         CLEAN.
    OUTCOME: 1 real fix (GPU latch — degraded performance, not visible
    corruption, which is why prior device logs looked fine: the FP5 just hadn't
    hit a transient GL failure in those boots). 7 categories confirmed clean with
    specific reasoning. On-device effect of the fix: after any future transient
    GPU-link failure, switching cameras (or restart) now recovers the GPU path
    instead of staying on CPU until app restart — watch for the absence of
    "forcing CPU path" persisting across a camera switch.

  - **★ CAMERA AUDIT (behavioral lenses, not static) — found + fixed a systemic
    "resume loses the user's camera choice" bug across SIX call sites.** USER
    asked for DIFFERENT ways of looking at the camera. Applied three lenses that
    static category-grepping can't: (1) device-log-as-ORACLE (does observed
    runtime match intent), (2) invariant/contract auditing (do callers honor what
    they assume), (3) cross-path consistency (do preview/photo/video agree).
    THE FIND (lens 1 → 2 → 3): device log showed a user on the FRONT camera
    (camera_10) navigate away and return → "start (default) → camera_29" (REAR).
    Traced to startCamera() (invoked from showEvent on every page return) calling
    the NO-ARG m_backend->start(), which resolves to defaultDeviceId()=
    primaryRear. So the user's explicit front-camera choice was silently reset to
    rear on every page return — even though the code's OWN contract (the
    firstFrameReceived handler comment) says "the selector is the single source
    of truth for what we asked for." Contract violated.
    CROSS-PATH (lens 3): the SAME no-arg start() appeared in 5 MORE resume paths —
    every "resume viewfinder after recording" site (onTranscodeFinished x2,
    onTranscodeFailed, the shutter-release resume, the deferred-resume lambdas)
    plus "devices appeared". So recording a video on the FRONT camera also
    resumed on REAR. One root cause, six manifestations.
    FIX: new private helper CameraWidget::resumeSelectedCamera() — starts the
    SELECTED device (m_cameraSelector->currentData()), falling back to the
    backend default ONLY when the selector holds the PIP sentinel or is empty (so
    we never pass the sentinel as a node). Routed ALL SIX resume sites through it
    (verified: zero bare ->start() resume calls remain; the only explicit
    start(node) calls left are intentional device-specific switches at 574/615/
    699). start(const QString& = {}) already supported the arg — the header
    comment at GstLaunchBackend.hpp:115 even documented the INTENT ("start() with
    an explicit deviceId so manual selection doesn't [get lost]") that the resume
    paths violated. CameraWidget.cpp + header compile under real Qt6+C++20.
    On-device check: flip to front (selfie), swipe to inbox, swipe back → should
    STAY on front (was: reset to rear). Same after recording a front-camera
    video. WHY PRIOR LOGS LOOKED FINE: the user happened to be testing rear most
    of the time, so default==selected and the bug was masked; the one front→
    away→back sequence in this log is what exposed it.

  - **★ FIX: link error on device — wired the menu+mailbox FFI staticlibs into
    CMake.** The unified-mailbox Qt code COMPILED (headers declare the FFI) but
    FAILED TO LINK: undefined references to sc_reqmb_new/free/receive/apply/
    pending/pending_count + sc_menu_actions_for, because nothing told CMake to
    build/link libsnapclone_mailbox.a and libsnapclone_menu.a. (My fault — I
    added the FFI calls without the build wiring last session.)
    FIX (followed the established saliency/group pattern exactly):
      * Root CMakeLists.txt: always-on blocks building snapclone-menu +
        snapclone-mailbox as STATIC IMPORTED GLOBAL libs via cargo into
        ${CMAKE_BINARY_DIR}/rust-target (snapclone_rust_menu_lib /
        snapclone_rust_mailbox_lib). Always-on, not behind a flag — the unified
        mailbox is core functionality.
      * src/ui/CMakeLists.txt: target_link_libraries(snapclone_ui PRIVATE
        snapclone_rust_menu_lib snapclone_rust_mailbox_lib) — InboxWidget is the
        live caller (unlike the group lib, which stays unlinked until its
        consumer lands).
    The final-exe link already has --allow-multiple-definition (src/CMakeLists.txt
    61-63) so the extra Rust std copies from two more staticlibs are handled.
    VERIFICATION: confirmed every undefined symbol from the error is (a) declared
    in its header (snapclone_requests_ffi.h / snapclone_menu_ffi.h) and (b)
    exported as a defined 'T' symbol by the .a (nm). Then REPRODUCED the failure
    scenario: compiled a probe calling all the error's symbols, linked it against
    BOTH staticlibs with the same --allow-multiple-definition flag CMake uses →
    LINKED CLEAN + RAN. Both crates build as the expected .a filenames
    (libsnapclone_menu.a, libsnapclone_mailbox.a). Device rebuild should now link.

  ========================================================================
  ★★ NEW DIRECTION (user, this session): GROUP CHATS as a "mini autoscrolling
  feed" — the core UX bet for dynamic group chats.
  ========================================================================
  VISION (user's words, paraphrased): group video snaps POP UP AND PLAY IN
  ORDER on the main page with a ~1 SECOND DELAY between them, like a mini
  auto-scrolling feed. The deliberate goal: get CLOSE to a live call, but
  trade the "constant stream you must actively participate in" for ROBUSTNESS,
  FIDELITY, and REACHABILITY. i.e. it's async-but-feels-live: each clip is a
  complete, high-fidelity, reliably-delivered snap (our existing strength —
  store-and-forward, encrypted, AV1/VVC), auto-played back-to-back so a burst
  of them feels like a conversation without requiring a held connection.
  WHY THIS FITS SNAPCLONE: we already do reliable encrypted snap delivery +
  transcode for fidelity; we do NOT do (and don't want) RTC's brittle held
  connection. The feed is the presentation layer that makes our reliable-async
  model FEEL synchronous. Reachability > latency.
  IMPLICATIONS / DESIGN QUESTIONS to resolve as we build:
    * ORDERING: clips play in order — by what clock? Sender timestamp is
      unreliable across devices; arrival order loses intent. Likely a
      per-group logical/Lamport-ish sequence (we already have group DAG
      machinery in snapclone-group — the causal order there is the natural
      source of truth). The feed playback order should follow the group DAG's
      causal sort, NOT wall-clock.
    * THE 1s DELAY: a gap between clips so it reads as a feed, not a jump-cut.
      Should be tunable; probably pause if the user interacts (scroll/tap),
      resume on idle. Must not block the UI.
    * BACKPRESSURE: if 10 clips arrive at once, queue + play sequentially with
      the delay, don't drop. If clips arrive while one is playing, append.
    * CATCH-UP vs LIVE: returning to the feed after being away — play unseen
      clips in order (catch-up) but maybe fast-forward the backlog? Open Q.
    * AUTOPLAY + FOCUS: only autoplay when the feed is the focused page;
      pause on navigate-away (mirrors the camera lifecycle we just fixed).
  BUILD PLAN (verifiable-first, per session pattern):
    1. [THIS PASS] Tested Rust model: a FEED QUEUE / PLAYBACK SCHEDULER —
       given a set of group clips with causal-order keys + a delay, produce the
       ordered playback schedule, handle late arrivals (insert in causal
       order even mid-playback if not yet passed), track seen/unseen, expose
       "what plays next + when". Pure logic, no Qt, fully testable. This is the
       brain; the Qt feed widget is a thin renderer over it later.
    2. C FFI for the scheduler (once shape is proven).
    3. Qt feed widget on the main page (device-bound; defer rendering specifics).
    4. Wire to the group DAG causal order as the ordering source of truth.
  ========================================================================

  - **TODO (user-requested this session, triaged):**
    * EDIT CONTACT — no implementation exists yet (ContactsPage/ContactCard have
      no editContact path). Concrete + compile-checkable. DOING THIS PASS.
    * AMBIENT MODE on the camera page — AmbientCameraDriver exists; wiring it as
      a camera-page mode is partly device-bound (visual). Investigate seam.
    * "266 video" (VVC/H.266 end-to-end) — referenced across VideoEncoderBackend/
      Transcoder/SnapRouter; needs ffmpeg --enable-libvvenc/libvvdec +
      SNAPCLONE_ENABLE_VVC=1 on device to verify. Device-bound.
    * GROUP CHATS — see the NEW DIRECTION block above (the feed UX).

  - **★ GROUP FEED SCHEDULER — tested model for the autoscrolling-feed UX
    (the brain). Plus EDIT-CONTACT persist fix.** Two pieces this pass:
    (1) EDIT CONTACT — was NOT a missing feature; openEditor + ContactEditorDialog
        already exist and open. The real bug: the dialog opens non-blocking via
        open() (not exec()), so the documented "read editedContact() after exec()"
        never ran — onSave() updated the dialog's internal copy and accept()ed,
        but NOTHING read it back, so a renamed contact silently reverted. The
        persist plumbing already existed (ContactsPage::contactEdited →
        MainWindow → StorageManager::saveContact). FIX: connected the dialog's
        QDialog::accepted to read editedContact(), update m_contacts, emit
        contactEdited, rebuildCards(). Note: the editor edits `nickname`, and
        Contact::label() prefers nickname over displayName — so editing nickname
        IS the user-facing rename (displayName stays as the peer-advertised name,
        correctly not locally clobbered). ContactsPage compiles. The one missing
        emission was the whole bug.
    (2) GROUP FEED SCHEDULER (snapclone-group::feed, NEW, tested) — the data-model
        brain for the "mini autoscrolling feed" group UX (see NEW DIRECTION block
        above). FeedScheduler: pure logic, time injected (no clock/timer/IO), so
        the Qt feed widget is a thin renderer later. tick(now_ms)->FeedState
        {Idle|Play(id)|Playing(id)|Gap{next,ms_remaining}|Paused}. add_clip
        inserts in CAUSAL ORDER (order_key = rank in the group DAG's
        total_order, which sorts by (lamport,author,id) — every replica plays the
        SAME sequence; we do NOT order by wall-clock arrival). Handles: the ~1s
        inter-clip gap (tunable gap_ms); LATE arrivals slotting into causal
        position if not yet passed (even mid-feed); seen-tracking (no replay,
        catch-up skips seen); pause/resume with time REBASE (paused feed resumes
        where it left off, doesn't skip clips for elapsed wall time); focus-gating
        (unfocused page → Idle, no autoplay — mirrors the camera-lifecycle fix);
        unknown-duration clips wait for an explicit mark_finished from the
        renderer. 7 tests, all green (group crate 59+1). ALIGNMENT: consumes the
        existing dag::total_order causal ordering as the playback-order source of
        truth — feed sequences + times, DAG decides what-precedes-what.
        NEXT RUNGS (per build plan): C FFI for the scheduler → Qt feed widget on
        main page (device-bound rendering) → wire to live group DAG. The C++
        group SESSION over snapclone-group transport is still the prerequisite
        for real group clips flowing in.
    TRIAGED-BUT-DEFERRED (device-bound, noted in TODO): ambient-mode-on-camera
    (AmbientCameraDriver exists; wiring is visual), "266 video"/VVC end-to-end
    (needs ffmpeg libvvenc/libvvdec + SNAPCLONE_ENABLE_VVC=1 on device).

  - **★ DEVICE-LOG VERDICTS + two evidence-driven fixes (negotiation recovery
    corrected; transcode-on-openh264 fix).** New device logs (two phones) gave
    verdicts on prior work AND surfaced the real camera issue.
    VERDICTS:
      ✓ GPU-latch reset WORKS: logs show GPU path healthy, later "CPU colorspace
        (GPU disabled by policy)" after failures — so it latched, and my
        node-change reset brings GPU back on the next session. Confirmed.
      ✓ Double-tap flip, tap-to-focus, LAN announce (192.168.2.224 now, not the
        usb0 172.16 — announce-ranking fix HELD), preview layout all clean.
      ✗✗ MY RELAXED-CAPS NEGOTIATION FIX WAS WRONG. Device proof at 16:00:01.880:
        the relaxed pipeline (format=BGRx, NO width/height pin) STILL failed
        not-negotiated (-4) 100ms after the first failure. So caps were never the
        cause — not-negotiated on the FP5 is a TRANSIENT DEVICE-STATE failure
        (libcamera/pipewire hasn't released+reset the sensor yet), strongly
        correlated with PIP (two concurrent libcamera opens) and fast stop/start.
    FIX 1 (camera negotiation recovery, REVISED): replaced the "relax caps, retry
      in 100ms" first-response with a DEVICE-SETTLE retry: on not-negotiated, wait
      kNegotiationSettleMs=600ms then retry the SAME node with NORMAL caps (give
      libcamera time to free the device). Only if THAT still fails do we try
      relaxed caps, then swap nodes. New m_negotiationSettleRetried flag (one
      per session, reset on node-change like the others; NOT reset on our own
      same-node re-entry so it can't loop). The old 100ms was simply too short.
    FIX 2 (transcode of openh264enc recordings): device logs showed an
      openh264enc-recorded .mkv FAILING every gst transcode backend (svtav1/vp9/
      vp8 all hit matroskademux not-negotiated) → raw .mkv fallback sent
      (bigger, not AV1). The x264enc phone transcoded fine. Root cause theory
      (device-bound, can't run FP5 gst here): openh264enc byte-stream parsed to
      AVC without periodic SPS/PPS → matroska track not self-describing on
      re-demux. FIX: openh264enc's parser is now "h264parse config-interval=-1"
      (re-inserts SPS/PPS before every keyframe). Also fixed the arg-append to
      SPLIT the parser string into tokens (was appended as one arg, would've
      broken the pipeline). Note: this is the gst-transcode branch (ffmpeg
      absent); the ffmpeg phone is unaffected.
    BOTH compile under real Qt6+C++20. Both are device-bound for final proof:
      Fix1 watch — "device-settle retry (SAME node, normal caps) in 600 ms" then
      a first frame instead of a cascade; Fix2 watch — openh264 phone's recording
      transcoding to .webm (av1/vp9) instead of "no further backends → raw .mkv".
    HONESTY: I previously shipped the relaxed-caps theory as the negotiation fix;
      the device disproved it. Corrected here rather than left standing.
    STILL OPEN (noted): PIP starting two concurrent libcamera opens is the likely
      TRIGGER for the device-state corruption — a deeper fix would serialize or
      avoid concurrent opens of the same media device, but that needs device
      iteration. Front-camera post-flip stall (framedrop watchdog exit 9 at
      16:02:47) may be the same root cause.

  - **★ REAL BUG FOUND BY THE NEW COMPILER: AppController.cpp misordered
    #endif/brace (config-specific, brace-check could NOT catch it).** "keep
    working" → went back and ran PRIOR-session C++ (only ever brace-checked)
    through real g++. Most compiled clean: SwipeNavigator.cpp (the scrub fix +
    overscroll tuning — the highest-risk untested file, 3426 lines, ✓), plus
    ContactCard, ContactsPage, CameraRegistry, QtCameraBackend, CameraPolicy,
    UnifiedNavBar, ScrollAreas, InboxWidget, FocusSweep, VideoRecorder,
    MicroDhtClient, GossipDiscovery, DirectDiscovery, NetworkManager, main.cpp,
    MainWindow, SettingsPage, AppController_connect/_commands, FirewallPortManager
    — all ✓ (some needed moc pre-generated via /usr/lib/qt6/libexec/moc for TUs
    that #include "<name>.moc", and extra module paths: QtWebSockets, QtDBus).
    THE BUG: AppController.cpp ~636-647, inside Private::start(). An
    `#if defined(SNAPCLONE_OPENDHT_BACKEND)` block opened `if (auto *dht=...) {`
    but the `#endif` was placed BEFORE that if's closing `}` — so with OpenDHT
    ON it balanced, but with OpenDHT OFF the preprocessor removed the if-body and
    left a STRAY `}` that closed Private::start() early → every statement after
    (d->network->start(...), pollinator setup, connect(...)) parsed at namespace
    scope → "'d' does not name a type" cascade. FIX: swapped the order so the `}`
    is INSIDE the `#if` (close the if, THEN #endif), making the whole gated block
    balanced whether OpenDHT is on or off. VERIFIED COMPILES BOTH WAYS now
    (-DSNAPCLONE_OPENDHT_BACKEND and without). This is precisely the bug class
    brace-checking is blind to — it doesn't evaluate the preprocessor, so it saw
    balanced braces with the #if content present. The default build (OpenDHT on)
    masked it; a non-DHT config build would have failed hard. Scanned the other
    OpenDHT-gated files (AppController_connect/_commands, MicroDhtClient) in the
    OFF config too — all clean, so this was isolated, not systemic.
    UPDATED RECIPE module list now includes QtMultimediaWidgets, QtConcurrent,
    QtSql, QtWebSockets, QtDBus; moc-including TUs need a pre-generated .moc on
    the include path (-I/tmp/mocgen). Config matrix matters: also check with
    OpenDHT OFF, since that path is least exercised.

  - **★★★ FULL APP BUILDS, LINKS, AND RUNS — first time end-to-end here. Found +
    fixed 4 real build bugs the per-file syntax checks could NOT catch.** "keep
    working" → pushed past per-file syntax checking to a real cmake
    configure-and-link of the DEFAULT config (OpenDHT OFF). Discovery: in the
    default config the ONLY hard external dep is Qt6 (GStreamer/PipeWire are
    subprocess CLIs, not linked; OpenDHT is OFF + from-source). Installed the last
    missing Qt modules (qt6-positioning-dev; QtWebSockets/QtDBus were already in
    base). `cmake -B build -DCMAKE_BUILD_TYPE=RelWithDebInfo` CONFIGURED clean
    (only XKB missing — runtime windowing, not needed to compile). CMakeLists
    already wires snapclone-menu as a real target ("Rust menu+mailbox: ON").
    THE 4 REAL BUGS (all config/version-specific → invisible to brace-check AND
    to single-file -fsyntax-only in the default flags):
      1. NatTraversal.cpp — qToBigEndian used 6× with NO #include <QtEndian>
         (was resolving transitively; broke under a real build). FIX: added the
         include. Version-independent genuine bug.
      2. QWebSocket::errorOccurred (4 sites: WebSocketTransport ×2,
         NetworkDiagnostics, RelayTransport) — that signal name is Qt 6.5+; the
         older Qt 6.4.2 here has the overloaded `error` signal. This is VERSION
         SKEW: the code is correct for the device's newer Qt. FIX: did NOT rename
         (would break the device build) — added include/network/QtWebSocketCompat.hpp
         with SC_QWEBSOCKET_ERROR_SIGNAL = (&QWebSocket::errorOccurred) on Qt>=6.5
         else QOverload<QAbstractSocket::SocketError>::of(&QWebSocket::error).
         Builds on BOTH, device behavior unchanged.
      3. DhtResourceManager.cpp — called m_dht->{stop,start,setReannounceInterval}
         on DhtDiscovery, whose full definition is #if SNAPCLONE_OPENDHT_BACKEND
         (only a fwd-decl in the default build) → "incomplete type". The .cpp is
         compiled UNCONDITIONALLY but its 3 deref sites weren't guarded. FIX:
         wrapped the 3 m_dht-> dereferences in #if defined(SNAPCLONE_OPENDHT_BACKEND)
         (m_dht can't be non-null without a backend anyway; surrounding uptime/
         stats bookkeeping left intact).
      4. AppController_misc.cpp sendShakV2 — dynamic_cast<Crypto::RustCryptoService*>
         but that header is #ifdef SNAPCLONE_RUST_CRYPTO (OFF by default) → type
         not named. The function's whole PQ body needs the Rust type; the code's
         own comment says it should no-op without PQ. FIX: #ifndef
         SNAPCLONE_RUST_CRYPTO → Q_UNUSED + debug log + early return; #else wraps
         the existing PQ body; #endif before the closing brace.
    RESULT: `cmake --build build` → [91%] Built target SnapClone. 91 MB
    executable at build/src/SnapClone. nm confirms the Rust staticlibs are LINKED
    IN: sc_menu_actions_for (this session's menu model), sc_saliency_bbox,
    snapclone_saliency::roi_plan all present as real symbols. RUNS headless
    (QT_QPA_PLATFORM=offscreen): prints "SnapClone 1.0.0", acquires the instance
    lock, initializes subsystems, runs DB schema migrations 1→2→3 OK.
    NEW RUNTIME BUG SURFACED (separate, deeper class — deferred): DB "Migration 4
    failed: no such column: last_seen" — a real migration-ordering/schema bug that
    only a RUNNING build could expose. Next session candidate.
    CAVEAT: this is the OpenDHT-OFF / RUST_CRYPTO-OFF default config on Qt 6.4.2;
    the DHT and PQ-crypto code paths (and Qt 6.6+ GPU camera APIs) still aren't
    compiled/linked here. But the full default app now builds+links+boots — a
    massive jump from "C++ not compilable here."

  - **★★ RUNTIME BUG FIXED: DB migration 4 ("no such column: last_seen") — app
    now boots fully to "All subsystems ready."** "keep working" → chased the
    runtime bug the running binary surfaced last step. ROOT CAUSE: migration 1
    creates peer_addresses(user_id, host, port) — NO last_seen. Migration 4 then
    did `CREATE TABLE IF NOT EXISTS peer_addresses (... last_seen ... fail_count
    ...)` — but since the table ALREADY existed from migration 1, IF NOT EXISTS
    made the CREATE a silent no-op, so last_seen/fail_count were never added, and
    the very next statement `CREATE INDEX ... ON peer_addresses (user_id,
    last_seen DESC)` failed with "no such column: last_seen". This bug was
    invisible to compile-checking — it only manifests at RUNTIME on a fresh DB.
    FIX: rewrote migration 4 to ALTER TABLE ADD COLUMN last_seen + fail_count
    (then the index). The migration runner (applyMigration) already detects
    ALTER...ADD COLUMN and skips idempotently when the column exists (its
    columnExists check), so this is safe on fresh, upgrade, AND already-repaired
    DBs. Used a CONSTANT default ('') not datetime('now') — non-constant defaults
    on ADD COLUMN are SQLite-version-dependent (the original CREATE could use it;
    ALTER can't portably). VERIFIED 3 ways: (1) sqlite3 sim of migrations 1→4 →
    table has both columns + index builds; (2) UPGRADE sim (v3 DB w/ old table +
    a real row → fixed v4) preserves the row (sc-old|10.0.0.5|9876||0); (3) the
    REAL BINARY on a fresh DB now runs ALL 14 migrations (1→14) to completion and
    reaches "All subsystems ready" — WebSocket server listening :9876, gossip on
    :9877 joined multicast, NetworkManager up (self sc-60dbda35), UDP channel,
    location updates. Remaining run-time warnings are ENVIRONMENTAL + expected:
    STUN/public-IP lookups fail (no internet to those hosts in the container),
    no firewall backend (reported gracefully), QPainter "painter not active"
    (offscreen platform rendering widgets with no display). So: full default
    build now COMPILES + LINKS + BOOTS to a listening, ready state headless.
    (Build-time fixes from the same session — QtEndian include, QWebSocket signal
    compat shim, DhtResourceManager + sendShakV2 config guards — all still in.)

  - **★★ RUST_CRYPTO=ON config now BUILDS + LINKS + RUNS — crypto path compiled
    for the first time in any session; fixed 3 real issues.** "keep working" →
    enabled SNAPCLONE_RUST_CRYPTO=ON to compile the C++ crypto paths (incl this
    session's sendShakV2 #else branch) that the default config never builds.
    ISSUE 1 — workspace getrandom/edition2024 drift (the project-long Rust
    blocker): traced it to `uuid` — the lock had drifted to uuid 1.23.1 which
    pulls getrandom 0.4.2 (needs edition2024 > cargo 1.75). The crates only ask
    for uuid="1". FIX: `cargo update -p uuid --precise 1.11.1` (last uuid on
    getrandom 0.2.x); removed getrandom 0.4.2 + a chain of edition2024 transitive
    crates from the lock. NOTE: cbindgen's build.rs runs `cargo metadata` over
    the WHOLE workspace, so even after the uuid pin other heavy siblings
    (tract-onnx/server stack) drag in another edition2024 crate (moxcms) →
    metadata still fails. The crate ships an escape hatch SNAPCLONE_SKIP_CBINDGEN=1
    (build.rs honors it; the C header include/crypto/snapclone_crypto_ffi.h is
    committed), purpose-built for "MSRV mismatch in transitive deps." With it set,
    snapclone-crypto COMPILES under cargo 1.75 (incl fips203 ML-KEM-768 +
    x25519-dalek) — first time.
    ISSUE 2 — vtable/typeinfo undefined for RustCryptoService at link. Root cause:
    the class had NO out-of-line virtual, so its vtable had no definite emission
    TU → "undefined reference to vtable/typeinfo". FIX: declared ~RustCryptoService()
    override in the header + defined it `= default` out-of-line in the .cpp (a
    classic vtable anchor).
    ISSUE 3 — even with the anchor, vtable stayed undefined because AUTOMOC had
    NOT generated moc_RustCryptoService.cpp (the QObject vtable slots metaObject/
    qt_metacast/qt_metacall were missing). Cause: I'd enabled crypto via an
    in-place `cmake -B build` reconfigure and AUTOMOC's file set was stale. FIX:
    clean reconfigure (rm -rf build) → AUTOMOC now emits moc_RustCryptoService.cpp.
    BOTH the anchor AND a clean configure were needed.
    RESULT: with SNAPCLONE_RUST_CRYPTO=ON (+ SNAPCLONE_SKIP_CBINDGEN=1, OpenDHT
    still OFF) the full app builds → [91%] Built target SnapClone, links clean
    (no undefined refs). nm confirms fips203/ML-KEM + RustCryptoService meta-data
    in the binary. RUNS headless: "Using Rust AES-256-GCM crypto backend",
    "Generated X25519 identity keypair for sc-87ca5855", "All subsystems ready."
    The Rust crypto + PQ integration path is now compiled AND boots.
    BUILD NOTE for this config: export SNAPCLONE_SKIP_CBINDGEN=1 before
    cmake --build, and do a clean configure when toggling RUST_CRYPTO so AUTOMOC
    refreshes. Lock now pins uuid 1.11.1 (Cargo.lock changed — affects all rust
    builds; zero-dep crates unaffected, the pin only lowers uuid's dep tree).
    STILL not compiled here: OpenDHT backend (from-source submodule, network-
    blocked) + Qt 6.6+ GPU camera APIs.

  - **DHT integration C++ now SYNTAX-VERIFIED in the OpenDHT-ON path (via an API
    stub) — incl this session's two AppController/DhtResourceManager fixes, now
    checked in BOTH configs.** "keep working" → tackled the last never-compiled
    config. Real OpenDHT can't build here (from-source git submodule, network-
    blocked — confirmed absent), BUT the project's DHT integration C++ only needs
    opendht's C API HEADER to compile, not the built lib. Wrote a faithful stub
    tools/dev-stubs/opendht_c.h declaring exactly the 11 functions + opaque types
    (dht_runner, dht_value, dht_infohash by-value, dht_data_view{data,size}) +
    callback typedefs (bool(*)(const dht_value*,void*) get, void(*)(bool,void*)
    done) the code uses, signatures reverse-engineered from the call sites to
    mirror opendht/c/opendht_c.h. RESULT (g++ -fsyntax-only -DSNAPCLONE_OPENDHT_BACKEND
    -I tools/dev-stubs): JamiAdapter_dht_impl.cpp (1121 lines, NEVER compiled in
    any session) ✓ COMPILES; JamiAdapter.cpp ✓; DhtResourceManager.cpp ✓;
    AppController.cpp ✓; AppController_connect.cpp ✓; AppController_commands.cpp ✓.
    IMPORTANT: this CLOSES THE LOOP on the two fixes I made earlier this session —
    AppController.cpp (misplaced #endif) + DhtResourceManager.cpp (m_dht-> guards)
    were verified to compile with OpenDHT OFF earlier; now verified ON too, so the
    SNAPCLONE_OPENDHT_BACKEND blocks I added are balanced + correct in BOTH
    directions. CAVEAT: syntax/type gate only — the stub has no implementations so
    it won't LINK, and real OpenDHT behavior is untested. But every line of the
    DHT integration now passes the compiler in the config where it's used. Stub +
    recipe saved under tools/dev-stubs/ for reuse. Coverage now: default config
    builds+links+runs; RUST_CRYPTO=ON builds+links+runs; OPENDHT_BACKEND=ON
    syntax-verified (can't link here). Only Qt 6.6+ GPU camera APIs remain wholly
    uncheckable (need newer Qt + device).

  - **★★ C++ TEST SUITE RUN (first time here) — found + fixed a real
    peer_addresses PRIMARY KEY bug; all 3 suites now GREEN.** "keep working" →
    ran the project's own C++ tests (ctest). Initially 1/3 failed:
    test_StorageManager::saveAndLoadPeerAddress — savePeerAddress() returned
    false. ROOT CAUSE (chained off the migration-4 saga): migration 1 created
    peer_addresses with `user_id TEXT PRIMARY KEY` (single col). Migration 4
    *intended* `PRIMARY KEY (user_id, host, port)` but its CREATE TABLE IF NOT
    EXISTS no-op'd (table already existed), so when migration 4 was later fixed to
    ALTER-add last_seen+fail_count, the columns appeared but the single-column PK
    SURVIVED. savePeerAddress() does an upsert `ON CONFLICT(user_id, host, port)`
    — which requires a UNIQUE/PK on exactly those 3 cols — so it failed with "ON
    CONFLICT clause does not match any PRIMARY KEY or UNIQUE constraint".
    FIRST ATTEMPT (reverted): added migration 15 to rebuild the table with the
    composite PK. FAILED at runtime: "database table is locked" — exactly the
    failure mode the original migration-4 comment warned about; a table rebuild
    (RENAME/DROP) inside applyMigration()'s transaction trips the lock.
    CORRECT FIX: extended repairSchema()'s existing peer_addresses rebuild (which
    runs OUTSIDE the migration transaction, each stmt on its own query, so no
    lock) to trigger not only when fail_count is missing but ALSO when the PK
    isn't the 3-column composite. Detect via PRAGMA table_info (count cols with
    pk>0; expect 3). Preserve data: capture whether the old table has fail_count
    BEFORE renaming (columnExists has a table-name allowlist that rejects the
    transient peer_addresses_old + would log a spurious QCRITICAL and take the
    data-losing copy branch), then INSERT...SELECT the right column set.
    VERIFIED: sqlite3 sim (1→4→rebuild → composite PK, upsert works, row
    preserved); test_StorageManager now 9/9 PASS; full ctest 3/3 PASS
    (test_CryptoService + test_GossipDiscovery also green); full app still builds
    [91%] + boots — boot log shows "repairSchema: peer_addresses PK has 1
    column(s), expected 3 — rebuilding for composite PK" then "All subsystems
    ready". This was a genuine latent runtime bug (peer-address caching upsert
    silently failing → every save returning false → addresses never persisted,
    forcing a DHT lookup every time) that only the unit test surfaced. The
    earlier migration-4 column fix + this PK fix together fully repair the
    peer_addresses schema across fresh and upgraded DBs.

  - **GROUP LIVE-COMPOSER decision model — snapclone-composer — Rust-TESTED (17)
    + FFI linked & RAN.** USER described a novel group-chat surface: a live
    composer showing the latest ~3 media as tiles; scroll/drag up/down to slot
    tiles in/out of a 3-up window to move back/fwd within the last 24h; when
    voice is identified in a tile it plays ALONE (the other two don't play audio
    — "one at a time"); tap any sub-window to replay (photo-gallery × realtime-DB
    × Snapchat feel). Separated the VERIFIABLE core (pure state math) from the
    untestable Qt glue. NEW crate rust/snapclone-composer (ZERO deps,
    staticlib+rlib). Composer::new(tiles, now) filters to trailing 24h + sorts;
    view_at/scrub/live_view/activate_id → ComposerView{ up to 3 VisibleTile
    (prev/active/next slots) each with PlaybackState, active_index, window_len,
    voice_gating }. PlaybackState: active=PlayingWithAudio; neighbors w/o voice=
    PlayingMuted; neighbors w/ active-voice=FrozenForVoice (the "one at a time"
    rule); stills=Still (a still can't carry/gate voice). scrub() takes signed
    detent steps from the gesture layer + clamps; activate_id() = tap-to-replay,
    falls back to live if the id aged out. **13 model + 4 FFI tests** incl voice-
    freezes-neighbors, no-voice-plays-muted, still-active-doesnt-gate, still-
    neighbor-stays-still-under-voice, 24h-exclusion, scrub-clamp, tap-vanished-
    id→live, single-tile, out-of-range-pins. C FFI (ffi.rs): sc_composer_view +
    sc_composer_activate_id (caller-owned ScVisibleTile[3] out, out_active_index/
    out_window_len, panic-safe + null-tolerant). Header
    include/composer/snapclone_composer_ffi.h passes gcc -fsyntax-only C AND C++.
    README + workspace member added; in-workspace `cargo test -p
    snapclone-composer` green. ★ Linked a C++ consumer against the real
    libsnapclone_composer.a and RAN it: no-voice→active plays AUDIO + neighbors
    MUTED; voice→neighbors FROZEN — exactly the described behavior, proven across
    the FFI boundary.
    ASSUMPTIONS baked in (ask returned questions unanswered; chose defaults to
    match the described feel; all trivially changeable via the tests): scrub is
    hybrid detent+momentum; the 3 = active±1 in time (not newest-3); voice →
    neighbors freeze on a still (not loop muted). DEFERRED (Qt/device): the group
    view widget itself, gesture→steps wiring (reuse snapclone-gesture), the 3
    QMediaPlayer panes + applying PlaybackState, the drag/scroll affordance + tap
    targets. Also still pending from this request: edit-contact inline submenu UI
    + camera fullscreen preview wraparound ambience (same pattern: tested model
    where one exists, then device-checkable Qt glue).

  - **COMPOSER model REFINED per designer answers + GEOMETRY layer added —
    Rust-TESTED (24) + dim/warp proven over FFI.** User answered the 4 design Qs
    (the ask-input tool glitched/echoed but the answers came in chat):
    (1) scroll = hybrid detent+momentum [already matched]; (2) the 3 tiles = "a
    geometry-aware rotatable T stack"; (3) voice → others "dim de-emphasize and
    timewarp" [NOT freeze]; (4) build composer first [already chose].
    CHANGE A (unambiguous, applied): replaced PlaybackState::FrozenForVoice with
    Dimmed{rate_pct} — voice-gated neighbors keep PLAYING but dimmed + time-warped
    + muted, rather than freezing on a still. VOICE_DIM_RATE_PCT=35 (0.35x,
    tunable). FFI: SC_PB_FROZEN_FOR_VOICE → SC_PB_DIMMED + ScVisibleTile gains
    rate_pct (100 normal; 35 when dimmed); pb_to_c now returns (state,rate).
    Header + both write-sites + tests updated. Re-linked C++ demo RAN: voice →
    neighbors "DIMMED rate=35%", active "AUDIO 100%" — proven over FFI.
    CHANGE B (working assumption, isolated): added a separable PrismGeometry
    module (rotatable 3-face prism = the "T stack"): poses(slots)→per-face
    {slot, angle_deg, is_forward}; forward_slot()→which slot the prism commits
    to; face_steps()→translate a spin into feed scrub steps. Deliberately
    INDEPENDENT of the feed/voice logic (ComposerView = which tiles + playback;
    PrismGeometry = where they sit), so changing the shape later is a localized
    edit. **7 geometry tests** (rest→active forward, ±120°→next/prev forward,
    angle normalization, short-window, rotation→scrub steps, empty). 24 tests
    total; in-workspace green; header C+C++ OK.
    ⚠️ STILL NEEDS THE DESIGNER (ask-input tool keeps glitching — please answer in
    chat): the EXACT "rotatable T stack" shape. Built it as a 3-face prism
    (Toblerone) as the working assumption, but the unanswered follow-ups were:
    (a) is it a T-LAYOUT (active center + one above + one beside), a TURNTABLE/
    carousel, a time-axis vertical→horizontal fan, or the prism I assumed?
    (b) does ROTATE scrub through time, or just reorder which of the 3 is active,
    or is rotate+drag two separate gestures? (c) should the model compute the
    geometry (it now does, via PrismGeometry) or should Qt own the 3D? The
    feed/voice/playback core is shape-agnostic and won't change with the answer;
    only PrismGeometry would be swapped. DEFERRED (Qt/device): the group view
    widget, gesture→rotation/steps wiring (reuse snapclone-gesture), the 3
    QMediaPlayer panes applying PlaybackState + rate, the 3D prism rendering.

  - **COMPOSER geometry CORRECTED: prism → 2D retiling window manager (designer
    clarified) — 28 tests.** Designer answered the 3 follow-ups (in chat; the
    ask-input tool kept echoing): the "rotatable T stack" is "more like a two
    small one big", rotate "mixes it up / optimizes viewport while keeping
    context open / buffers the eyes if three messages come at once", and "it's
    more like a 2D retile window manager" (NOT 3D). So: REMOVED the PrismGeometry
    module (wrong assumption) and replaced it with TileLayout — a 2D tiling
    engine. One BIG focused pane + the other present slots as SMALL peripherals
    ("two small, one big"). layout(slots, big_slot)→Vec<TilePlacement{slot, Rect,
    is_big}> in viewport px; SmallEdge{Right|Bottom} + auto_edge (wide→right
    column, tall/phone→bottom row); big_fraction (default 0.68) + gap. next_big()
    = the re-tile step the rotate gesture drives (cycles which slot is big,
    promoting a peripheral into focus, demoting the current). Independent of the
    feed/voice logic (ComposerView=which tiles+playback; TileLayout=where on
    screen). **11 layout tests**: two-small-one-big invariant (exactly one big,
    smalls < big area), non-overlap (bottom + right edges), big_fraction
    respected, retile promotes/cycles/wraps, missing-big-slot fallback,
    single-tile-fills-viewport, two-tile, empty. FFI added: sc_composer_layout
    (slots+big_slot+viewport+edge → ScTilePlacement rects) + sc_composer_next_big
    (rotate step). Header updated (C+C++ OK). **28 tests total** (13 feed/voice +
    4 view-FFI + 11 layout); in-workspace green. ★ Linked C++ demo RAN: 400x800
    phone vp → active BIG 400x539 on top, two smalls 196x253 side-by-side along
    bottom; rotate→slot+1 becomes BIG, others demoted — exactly "two small one
    big" + re-tile, proven over FFI. Combined with the earlier dim/warp change,
    the composer's full decision+layout core now matches the designer's described
    feel. DEFERRED (Qt/device): the group view widget, gesture→{scrub steps,
    retile} wiring, 3 QMediaPlayer panes applying PlaybackState+rate into the
    TileLayout rects with animated transitions between re-tiles.

  - **★★ CAMERA not-negotiated-on-RESUME bug FIXED (FP5 device logs 2026-05-28).**
    Two real device logs reviewed. Cameras are MOSTLY HEALTHY: rear (camera_29) +
    front (camera_10) both cold-start, deliver first frames @1280x720 (3686400
    bytes), preview layout clean, double-tap flip works, record→transcode→AV1
    send works end-to-end (34MB mkv → 135KB webm AV1, ~3x realtime). ONE
    reproducible failure isolated: rear camera_29 hits "Format negotiation failed
    / not-negotiated (-4)" on RESUME (rear→background→rear) — but the SAME sensor
    streams fine on cold start + 7 min later, and front camera_10 NEVER fails.
    Timeline: 19:17 cold ok; 19:17:57 flip-restart ok; 19:18:28 resume → FAIL
    (NO "device-settle retry" line); 19:18:57 probe → FAIL; 19:25:38 cold → ok.
    ROOT CAUSE: the V32.35 negotiation-settle retry (the correct first response to
    a transient -4) was being SKIPPED on resume. Why: its one-shot guard
    m_negotiationSettleRetried is reset ONLY inside `if (node != m_currentNode)`,
    but stop() never clears m_currentNode — so resuming the SAME physical sensor
    takes the node==m_currentNode branch, skips the reset, and inherits a
    guard=true left over from an earlier session's recovery. The next -4 then
    finds the guard set → settle-retry skipped → transient failure becomes
    TERMINAL (matches the logs exactly: gst exited 1/255, no retry line).
    FIX (V32.36): reset the per-session guards (m_negotiationSettleRetried,
    m_retriedRelaxed, m_relaxCaps) on any FRESH session — start() called while
    !m_active — not only on node change. To avoid reopening a retry loop during
    our OWN recovery relaunches (which also pass through start() while !m_active
    after the finished handler clears m_active), added a one-shot
    m_isRecoveryRestart flag: set right before each recovery singleShot
    (settle / relaxed-caps / GPU-fallback / quality-restart), snapshotted+cleared
    at the very top of start() so it survives early-return paths. The fresh-
    session reset is gated on (!m_active && !recoveryRestart). GstLaunchBackend.cpp
    COMPILES clean (real Qt6). Can't reproduce on-device here, but the logic
    matches the observed pattern precisely and is the minimal change.
    THREE-DEVICE GROUP TEST: "three devices" = three PEERS, not three cameras
    (FP5 exposes 2 sensors). This fix directly serves it — cycling phones through
    camera sessions during a group test hammers exactly the resume path that was
    failing. CARRIED (separate, for a laptop 3rd peer): CameraRegistry only
    matches libcamera_input.*/virtual-cam-* node names, so USB/v4l2 webcams on a
    desktop peer aren't enumerated (classify path noted in CameraRegistry.cpp).
    FUTURE UX (user-flagged, not yet built): timelining UX will be tuned heavily
    (screen-property dependent); an "autoreply" button — anyone in the window
    auto-starts a 5s reaction when a snap arrives, toggle off to stop the loop.

  - **GST pipeline LOGIC extracted to a TESTED crate — snapclone-gstpipeline (16
    tests) — locks down the resume-negotiation fix.** User asked for tests
    simulating a gst pipeline. Was honest about the split: real pipewire/
    libcamera negotiation is hardware and can't be meaningfully faked, but the
    backend's DECISION logic (the part the resume bug lived in) is pure and
    testable. NEW crate rust/snapclone-gstpipeline (ZERO deps): classify_error
    (stderr→GstError{Negotiation|GlLink|MissingNode|Other}, GL-before-negotiation
    precedence matching the C++) + RecoverySm, a faithful model of the recovery
    ladder + per-session guard lifecycle. on_start/on_stop/on_failure mirror the
    C++ member-flag transitions 1:1 (m_negotiationSettleRetried, m_relaxCaps,
    m_gpuConvertFailed, m_isRecoveryRestart, m_active, m_currentNode incl the
    stop()-doesnt-clear-current_node asymmetry that caused the bug). Ladder:
    GL-link→ForceCpuAndRetry(100ms); negotiation early-exit→Settle(600)→
    Relaxed(600)→SwapNode; non-early→GiveUp. **13 model + 3 FFI tests** incl the
    full ladder, GL latch (no infinite loop), and the headline
    resume_same_sensor_can_still_settle_retry regression test. ★ PROVED the test
    CATCHES the bug: reverted the fresh-session reset in a temp copy → that test
    FAILED at the exact assertion; restored → green. So it's a real guard, not a
    pass-anyway test. C FFI (opaque ScGstRecovery* handle: new/free/on_start/
    on_stop/on_failure + sc_gst_classify_error; panic-safe + null-tolerant).
    Header include/gstpipeline/snapclone_gstpipeline_ffi.h passes gcc C + C++.
    README + workspace member added; in-workspace green. ★ Linked a C++ consumer
    against the real libsnapclone_gstpipeline.a and RAN it replaying the FP5
    scenario: classify(not-negotiated)→Negotiation; session1→SettleRetry/600ms;
    RESUME→SettleRetry (the fix; the bug would give SwapNode) — proven over FFI.
    NOTE: the C++ GstLaunchBackend keeps its inline copy (now matching this
    model); driving it from the crate via FFI is the optional next step. Crate
    count now: composer(28), gstpipeline(16), menu(17), gesture(15), group(52+1),
    videosrc(34) + others.

  - **★★ ROOT-CAUSE: snaps not sending — NetworkManager advertised the USB-tether
    IP, not wlan0. FIXED + tested.** Device logs (2026-05-28): two phones on the
    same WiFi, neither could send. BOTH announced "ws://172.16.42.1:9876" and
    every snap logged "No active session — queuing" then pending-ACK forever.
    172.16.42.1 is the FP5's usb0 USB-tether interface; the real WiFi subnet is
    192.168.2.x (gossip JOINs there). ROOT CAUSE: NetworkManager::start (~line
    126) resolved the self-advertise address with a naive "first non-loopback
    IPv4, break" loop. QNetworkInterface::allAddresses() lists usb0 BEFORE wlan0
    on the FP5, so it advertised the unreachable tether IP — each peer then tried
    to reach the other at 172.16.42.1 (not on the WiFi subnet) → session never
    opened → snaps queued forever, no ACK. The project ALREADY had the fix
    (AddressClass::pickBestLanAddress, ranks 192.168=100 > 10=80 > 172.16
    tether=40 > 169.254=10) but it was only invoked by GossipDiscovery::
    announcePresence AND only when handed "0.0.0.0" — NetworkManager passed a
    concrete (wrong) IP, so the correction was skipped and the else-branch took
    it verbatim. FIX: NetworkManager now builds the IPv4 candidate list and calls
    AddressClass::pickBestLanAddress itself (falls back to first candidate then
    127.0.0.1), so it advertises the reachable wlan0 address + logs it. The
    queue→flush path (NetworkManager ~line 478) already exists, so the snaps that
    were stuck will flush once the session opens with the right address.
    NetworkManager.cpp COMPILES (real Qt6). NEW C++ TEST tests/
    test_AddressClassification.cpp (Qt Test, 7 cases) pins the ranking incl
    wlanBeatsUsbTether (the exact 172.16-vs-192.168 FP5 case), order-independence,
    192>10>172>169, public/CGNAT/junk→0. Full ctest now 4/4 suites green
    (Storage, Crypto, Gossip, AddressClassification). Can't reproduce the
    two-phone topology here, but the root cause is unambiguous in the logs and the
    advertised address is now provably the ranked LAN one. NEXT on-device check:
    both phones should now announce ws://192.168.2.x:9876 and a session should
    open (watch for the "No active session — queuing" line to STOP and an ACK to
    arrive).

  - **COMPOSER screen-adaptation layer added — Rust-TESTED (36 total).** User
    flagged timelining UX will be tuned heavily + "screen properties will affect
    this." Added ScreenProfile to snapclone-composer: from_size(w,h) derives the
    layout params from viewport geometry — ScreenClass{Phone<480 short-edge|
    Tablet<900|Desktop} + Orientation{Portrait|Landscape}. Adapts: edge (portrait
    →Bottom row, landscape→Right column), big_fraction (Phone 0.72 > Tablet 0.66
    > Desktop 0.60 — small screens let the active tile dominate, big screens can
    shrink it since peripherals stay legible), max_visible (very narrow portrait
    phone <360 short-edge drops to big+1 instead of two-small-one-big).
    layout_for(viewport)→TileLayout + clamp_slots() (keeps active + nearest
    neighbor when over max). **8 screen tests** incl phone-portrait-bottom,
    landscape-right, big-fraction-shrinks-with-size, breakpoints, tiny-phone→2,
    clamp-noop-within-max, profile→TileLayout round-trip, square→landscape
    tie-break. The point: the screen-dependent tuning the designer will iterate
    on now lives as pure thresholds with a test safety net, not scattered Qt
    magic numbers. 36 composer tests total (feed/voice 13 + view-FFI 4 + layout
    11 + screen 8). DEFERRED (Qt/device): the group view widget reading
    ScreenProfile on resize/rotate + applying it. Conversation-view core is now:
    feed/voice/playback + 2D retile + screen-adaptation, all tested; remaining is
    rendering glue.

  - **★★ CONNECTION root cause #2 (the REAL one): sendToPeer never initiated a
    connect — FIXED + builds/links.** Device logs (00:05–00:09, builds 371d04d/
    9abe131, PRE my fixes): both phones now advertise CORRECT wlan0 addresses
    (ws://192.168.2.223 + .224 — so the address fix was right), yet every snap
    STILL logs "No active session — queuing". NEW evidence: DHT connection-request
    blobs DO arrive ("Connection request blob from sc-... — passing to
    AppController", repeatedly), but no session ever opens; gossip shows JOINs
    SENT to 192.168.2.255 but no "New peer discovered" (JOINs apparently dropped
    by the AP / client isolation, or only visible in the filtered-out [D] lines);
    DHT lookups time out. ROOT CAUSE found by tracing: NetworkManager::sendToPeer
    (~line 190), when there's no active session, only QUEUED the payload + fired
    udpSendNeeded — it NEVER called connectToPeer. The ONLY path that opens a
    session is gossip peerDiscovered → onPeerDiscovered → connectToPeer. So if
    discovery doesn't fire on a given network, sending a snap to a known peer does
    nothing but queue forever — there's no "I want to send, I know your address,
    so connect now" path. FIX: added NetworkManager::connectionNeeded(peerId)
    signal, emitted by sendToPeer when it queues (guarded by !session.connecting);
    AppController wires it to connectByDhtId(peerId) (which consults the cached
    LAN address + drives LAN/DHT/punch), debounced per-peer (kConnectKickDebounceMs
    =4s, lastConnectKickMs map) so a burst of queued snaps = ONE connect attempt.
    Now SENDING a snap actively establishes the connection instead of waiting
    passively for discovery. Files: NetworkManager.hpp (signal), NetworkManager.cpp
    (emit), AppController.cpp (wire+debounce), AppController_p.hpp (map),
    Timings.hpp (constant). FULL APP BUILDS + LINKS ([89%] Built target SnapClone).
    ctest 4/4. NEXT on-device check: after sending a snap, expect "[CONNECT] Snap
    queued for sc-X — actively establishing connection" then a connectToPeer
    attempt to the 192.168 address; the queue should drain once it connects. If it
    STILL fails, the connect is reaching the peer but the handshake/ACK is failing
    — that's the next layer to look at (and we'd want UNFILTERED logs: the user's
    `rg bound` filter hides snapclone.gossip [D] "New peer discovered" / "Gossip
    packet received" and the connectToPeer attempt lines).
  - **CAMERA resume bug: logs RECONFIRM it, fix is in tree but NOT yet built by
    user.** Same logs: camera_29 fails not-negotiated on resume at 00:08:48
    (exit 1) + 00:08:58 (exit 255), ~100ms each, NO "device-settle retry" line —
    exactly the pre-fix behavior. The V32.36 fix (settle/relaxed recovery moved
    OUT of the !m_retriedAlternateNode-gated block, so it runs even when the user
    explicitly picked the node on resume) is committed in GstLaunchBackend.cpp
    (compiles) but these logs predate it. User must REBUILD to test. Post-rebuild,
    expect the "device-settle retry (SAME node, normal caps)" line + recovery on
    the camera_29 resume instead of a terminal exit.

  - **GROUP-SHARED DHT MAILBOX — "share the slot, not the key" — implemented +
    tested + doc'd for rollback.** User asked: can mailbox slots be shared but
    secure to bulk up comms / save traversal, without hurting DHT resiliency?
    Analysis: (1) resiliency is NOT the risk — OpenDHT over-replicates popular
    keys, so a shared key helps availability; (2) the real risk is crypto — a
    shared SYMMETRIC group key would kill forward secrecy + revocation; (3) the
    right design is per-sender sealing under one shared SLOT (Signal/MLS sender-
    keys style). VERIFIED the multi-writer foundation already works: OpenDHT
    stores distinct values side-by-side under one key (publishConnectionRequest
    uses dht_value_new auto-id; checkConnectionRequests GET iterates ALL values,
    dedups replicas) — N senders coexist, confirmed in JamiAdapter_dht_impl.cpp.
    The group crate ALREADY had the sender-key primitives (ratchet::SenderChain
    fwd-secrecy + seal::seal_sender_key/encrypt_content). BUILT: new
    rust/snapclone-group/src/mailbox.rs (pure, tested): MailboxMode::from_flag
    (PerRecipient default/rollback | GroupShared), group_slot_key (group/groupmsg),
    MailboxEntry to_bytes/from_bytes (plaintext addressing header + sealed body,
    2048 cap matching the C++ GET cap), SlotReader (dedup replicas + filter group
    + order by (sender,seq) stable across peers + per-sender gap detection),
    build_entry (seal via SenderChain message key). **10 mailbox tests + 4
    mailbox-FFI tests** (group crate 73 total, was 69). FFI: sc_mailbox_mode_from_flag
    / _slot_key / _build_entry / _parse_entry added to ffi.rs + header
    include/group/snapclone_group_ffi.h (C+C++ OK). ★ Linked C++ demo against the
    real libsnapclone_group.a + RAN: mode(off)=PerRecipient, slot key built,
    Alice's entry → 24 wire bytes, reader parsed back sender=sc-alice seq=42
    sealed recovered byte-exact. LOGGING (user-requested): JamiAdapter_dht_impl.cpp
    GetCtx now counts distinct-coexisting-values + collapsed-replicas; logs
    "Mailbox check complete for X — distinct values: N (replica copies collapsed:
    M)" on every GET — N>1 confirms multi-writer coexistence on real hardware.
    Compiles with OpenDHT stub; FULL DEFAULT APP BUILDS+LINKS ([89%] Built target
    SnapClone). ROLLBACK DOC: docs/group-mailbox.md — gated by ONE runtime flag
    QSettings [network] groupMailboxShared (default false → per-recipient, the
    shipping path, untouched); set false to roll back, no rebuild, no migration.
    DEFERRED (C++ integration, untestable here): the actual PUT/GET on the group
    slot wired through publishGroupMessage/checkGroupMailbox calling the
    sc_mailbox_* FFI, gated on the flag — the per-recipient path stays the default
    until the hardware test validates the shared path.

  - **EDIT-CONTACT "no UI" — ROOT-CAUSED + FIXED (real bug).** User: edit contact
    button still shows no UI. Found it in ContactsPage::openEditor: a leftover
    from the exec()→open() migration. After dlg->open() (non-blocking), the code
    still ran the old exec()-era tail: `const int result = QDialog::Rejected; ...
    dlg->deleteLater();` — so deleteLater() was scheduled on the VERY NEXT event
    loop pass, destroying the editor a millisecond after it became visible. That's
    the "shows no UI" symptom exactly: open() shows it, immediate teardown kills
    it. ALSO m_openEditor (the stacked-dialog guard) was never cleared on a real
    close (no finished hookup), so any non-immediate-delete path left the guard
    stuck → subsequent taps silently "raise" a dead dialog. FIX: removed the dead
    exec()-era block; added connect(dlg, &QDialog::finished, ...) that clears
    m_openEditor and deleteLater()s the dialog on ACTUAL close. Acceptance/persist
    already handled by the accepted() connection. ContactsPage.cpp COMPILES; full
    app builds + ctest 4/4. Both edit paths (inline editBtn + ContactCard "Edit
    contact" submenu action) route through openEditor, so both are fixed.
  - **TOUCH SCROLL "inverse bouncy" — DIAGNOSED (not blindly flipped) + logging
    added.** User: tap-drag scroll on touchscreen does inverse/bouncy stuff.
    Traced the ENTIRE sign chain in SwipeNavigator: drag dx/dy = cur - touchStart
    (positive = finger right/down); commitScrollDelta does value -= d (finger down
    → value drops → content follows = natural); fling reuses the same convention;
    overscroll has a deadzone + rubber-band. The path is SELF-CONSISTENT — there
    is no global inversion in our code to flip. More likely cause: a COMPETING
    native/OS scroll handler double-acting on the same gesture (SettingsPage sets
    WA_AcceptTouchEvents on its scroll area+viewport at lines 893/1052;
    ContactsPage does NOT — so behavior can differ per page, and a page with both
    our filter AND Qt's built-in touch scroll would double-move = bouncy/inverse).
    Did NOT blind-flip a sign (would invert correct behavior). INSTEAD added a
    targeted diagnostic in commitScrollDelta: logs "[SCROLL-INVERT?] dy=N but value
    moved +M (same sign — expected opposite). Possible competing native scroll
    handler." when the scrollbar moves the SAME direction as the finger (the
    inversion signature). Next device capture will show definitively whether the
    inversion is in our mapping or a competing handler — then the fix is targeted.
    SwipeNavigator.cpp COMPILES; full app builds.
  - **CAMERA: re-examined, fix is correct + complete for what logs show; no new
    speculative change.** Re-checked the resume failures: the stop→resume gap is
    ~4.5s (plenty for sensor release), so it's NOT a release race a proactive
    wait would fix — it's a transient cold-negotiation the reactive settle-retry
    (already in tree, V32.36, gstpipeline crate mirrors it, 16 tests green) is the
    right tool for, and the same sensor succeeds moments later in every log. Adding
    more camera logic now would stack unverified changes; the fix just needs the
    user to REBUILD. Held off on over-engineering per the build-the-verifiable-rung
    principle.

  - **LAN connect-decision extracted to a TESTED model — snapclone-net-intel
    lan_decision (9 tests) — and CAUGHT 2 of my own wrong assumptions.** The
    decision NetworkManager::connectToPeer makes per outbound attempt (attempt
    LAN-direct / skip to relay / refuse) governs whether a queued snap ever
    connects — load-bearing for the send path I fixed last session, but it lived
    untested in Qt members. Extracted to rust/snapclone-net-intel/src/
    lan_decision.rs: decide(AddrClass, ConnType, is_self, lan_failures) →
    ConnectDecision{AttemptDirect | SkipToRelay(reason) | Refuse(reason)}. ★ The
    value of verifying against SOURCE not memory: my first draft got TWO things
    wrong vs the actual C++, caught by re-reading connectToPeer: (1) I had CGNAT
    skipping to relay — but the C++ only LOGS a hint and STILL attempts (relay
    preference is a downstream decision); fixed to AttemptDirect. (2) I only
    refused Loopback+LinkLocal — but the C++ refuses Invalid/Multicast/Broadcast
    too; added them. AddrClass now maps 1:1 onto AddressClass::Class (Invalid=0,
    Loopback=1, LinkLocal=2, Rfc1918=3, CarrierGrade=4→Cgnat, Public=6,
    Multicast=7, Broadcast=8). Threshold logic mirrors the cellular-aware guard:
    lan_threshold = 1 on cellular (LAN can't work over cellular), 2 otherwise
    (transient drops shouldn't lock out the session); RFC1918 attempts until
    lan_failures >= threshold then SkipToRelay(LanIsolationSuspected). **9 tests**
    incl self-beats-class, the refuse set, cgnat-still-attempts (the corrected
    one), wifi-vs-cellular thresholds, first-attempt-tries. net-intel 18 total
    (9 pre-existing + 9). In-workspace green; full app builds. This is now the
    tested SPEC the C++ guard is checked against; an FFI to drive the C++ from it
    is an optional next step (the C++ inline guard already matches).

  - **★ NETWORK COMPRESSION — content-aware wire codec, Rust-TESTED (15) + proven
    5× on chat text over FFI.** User: compression will make/break conversations in
    hard-to-service areas. Surveyed the wire: DHT mailbox payloads are 65-150B
    (correctly left raw); the bulk is the MEDIA body in SnapRouter::serializeSnap
    (QDataStream envelope: small header + `media`). Key realization: AV1 video +
    JPEG images are ALREADY compressed — blanket compression wastes CPU and can
    expand them. So built CONTENT-AWARE: new crate rust/snapclone-wirecodec
    (dep: miniz_oxide, pure-Rust deflate, builds offline). 1-byte codec tag
    (0x00 Stored | 0x01 Deflate) + encode/decode. Policy: Image/Video→Stored (no
    deflate attempt); Text/Link/Location/Unknown→try deflate, keep only if
    strictly smaller; <64B→Stored. GUARANTEES: lossless (decode(encode(x))==x,
    tested across all content kinds incl empty/tiny) + NEVER expands beyond +1
    byte (tested over many sizes/types incl high-entropy xorshift data). **11 lib
    + 4 FFI tests.** ★ The bad-test-data catch: my first "incompressible" test
    used patterned ints that deflate DID shrink → test correctly failed (proved
    the policy KEEPS compression when it helps); fixed the test to use xorshift
    high-entropy bytes that genuinely can't compress → fallback-to-Stored path now
    exercised. FFI sc_wire_encode/decode (caller-buffer + out_len + BufferTooSmall
    retry); header include/wirecodec/snapclone_wirecodec_ffi.h (C+C++ OK). Content
    class maps Core::MediaType (Image=0…Location=4, -1=Unknown). ★ Linked C++ demo
    RAN: realistic 312B chat caption → 63B on wire (20%, ~5× smaller), decoded
    byte-identical; 4096B pseudo-AV1 → stored verbatim (+1 tag, no wasted CPU).
    README + workspace member added. DEFERRED (C++ integration, version-gated):
    SnapRouter wraps the media bytes through sc_wire_encode before serialize /
    sc_wire_decode after deserialize — kept separate so the on-wire change can be
    a v5 wire version (back-comat) and validated on a hardware test before default-on.
    Crate count: wirecodec(15), group(73), composer(36), net-intel(18),
    gstpipeline(16) + others.

  - **★ WAYFINDER near-realtime usefulness — position FRESHNESS/CONFIDENCE model,
    Rust-TESTED (14) + proven over FFI.** User: keep working on location
    databasing + the wayfinder's near-realtime usefulness. Audited the chain:
    DATA LAYER IS HEALTHY — peer_locations has recorded_at (indexed) + accuracy_m,
    refreshed on snap receipt, pruned after 7 days (purgeStaleLocations, called
    from AppController:1612). The GAP: the wayfinder draws bearing/distance
    whenever a location exists AT ALL (WayfinderPage ~353), with ZERO regard for
    fix age/accuracy — a 3-hour-old fix gets the same confident arrow as a 30s
    one. A confident arrow at a stale position misleads (worse than none). Built
    the missing CONSUMPTION layer: new module rust/snapclone-location/src/
    freshness.rs — confidence(policy, age_secs, accuracy_m) → Confidence{Live≤2m
    +precise | Recent≤10m or imprecise | Stale≤1h or coarse | LastKnown}. Two
    trust rules: accuracy NEVER improves the age tier (precise 2h fix is still
    LastKnown); coarse caps freshness (5km IP-geo fix, even 8s old, capped at
    Stale — NOT a confident arrow). presentation()→SolidArrow/FadedArrow/
    LastSeenLabel; opacity()→1.0/0.85/0.55/0.35 (monotonic, tested); 
    last_seen_label()→"15s/5m/3h/2d ago". Thresholds in FreshnessPolicy (tunable;
    FFI _ex variant takes them for config). **10 model + 4 FFI tests** incl age
    tiers, coarse-caps-fresh, accuracy-never-improves-age, boundary-inclusive,
    negative-age-clamp (clock skew), monotonic opacity. FFI sc_loc_confidence/
    _ex/_presentation/_opacity/_last_seen_label; header include/location/
    snapclone_freshness_ffi.h (C+C++ OK); crate-type gained staticlib. ★ Linked
    C++ demo RAN across realistic ages: Alice 15s→LIVE/SolidArrow/1.00; Bob 5m→
    RECENT/Faded/0.85; Carol 35m→STALE/Faded/0.55; Dave 3h→LAST_KNOWN/LastSeen/
    0.35 "3h ago"; Eve 8s-but-5km→STALE (the coarse demotion). location 17 tests
    total; full app builds. Doc: snapclone-location/FRESHNESS.md w/ the exact
    WayfinderPage::refreshLayout integration snippet. DEFERRED (Qt presentation):
    wire the tier into the node paint (showArrow + opacity + last-seen label
    instead of distance) — pure rendering, no data migration, validates on-device
    where arrows are visible. Crate count: location(17), wirecodec(15), group(73),
    composer(36), net-intel(18), gstpipeline(16) + others.

  - **★ MOTION MODELLING (accel + vehicle/footsoldier) for wayfinder projection —
    Rust-TESTED (13 new, 34 crate total) + proven over FFI. Honest about physics.**
    User asked: can we project peer position with accelerometer + vehicle/foot
    modelling? FIRST established the hard constraints (no hand-waving): (1) NO
    sensor plumbing exists yet (no QAccelerometer anywhere); (2) peer_locations is
    PRIMARY KEY peer_id = ONE row, OVERWRITTEN — no location history to trend
    from; (3) PHYSICS: a peer's accelerometer can't locate that peer remotely —
    dead reckoning is SELF-tracking, so it's the SENDER enriching its own outbound
    position (peer-cooperative), not something the receiver computes; (4) raw
    double-integration of accel diverges as ½·b·t² (~tens of m in <1min) — phones
    never navigate that way. So built the REALISTIC model, not the naive one:
    mode-dependent. New rust/snapclone-location/src/motion.rs: classify_activity
    (still/foot/vehicle from MotionSummary{accel_std, cadence_spm, speed_mps} —
    activity recognition, tractable) + project() — FOOT: steps×stride along
    heading (PDR, ~8% error, bounded per-step not quadratic); VEHICLE: speed×time
    along heading (~20% error, turns unseen); STILL: don't move the dot. Returns a
    Projection with an uncertainty_m that GROWS with distance (fix error + proj
    error in quadrature) so the wayfinder widens the node halo honestly. Added
    haversine::destination (forward geodesic projection) + round-trip tests.
    **13 new tests** (classify still/walk/run/vehicle/ambiguous; project foot/
    vehicle/stationary; uncertainty-grows-with-distance; negative-input-safe;
    destination round-trips). FFI sc_motion_classify + sc_motion_project; header
    include/location/snapclone_motion_ffi.h (C+C++ OK). ★ Linked C++ demo RAN a
    realistic 45s-stale scenario: walking peer→projected 43m, ±9m halo (tight);
    driving peer→projected 630m N, ±126m halo (honest: moved far but fuzzy);
    parked→0m, ±8m; sender-classify tagged desk/walk/highway correctly. The big
    vehicle halo is the POINT — useful (dot moved the right way) + honest (don't
    trust the exact spot). location crate 34 tests; full app builds. DEFERRED
    (device + protocol, can't be faked here): (a) QAccelerometer/QCompass sampling
    → MotionSummary device-side; (b) a peer_location_history table (or motion
    columns on peer_locations) since there's no history today; (c) wire MotionState
    into the snap location payload (sender enriches outbound) + a wire-version
    bump; (d) wayfinder reads the projection + draws the growing halo. All the
    DECISION logic is tested; the sensor IO + transport + paint are the device
    rungs. Crate totals: location(34), wirecodec(15), group(73), composer(36),
    net-intel(18), gstpipeline(16) + others.

  - **★ EDIT-CONTACT DIALOG — visibility ROOT-CAUSED + fixed, AND UI modernized to
    redesign tokens. (User's stated biggest issue.)** Two distinct problems:
    (1) NOT VISIBLE ON OPEN — root cause found. ContactEditorDialog extends
    MobileDialog, an IN-WINDOW SHEET: it parents a scrim to the top-level window
    and slides ITSELF up as a CHILD widget (pos anim (0,root.h)→(0,root.h-dlg.h)).
    MobileDialog::open() already runs animateSheetIn() (scrim show+raise, slide,
    15s stuck-timer) then QDialog::open(). But ContactsPage::openEditor was
    SABOTAGING it with leftover V32.32 "belt-and-braces": setWindowFlag(
    WindowStaysOnTopHint) FORCED the dialog into a separate top-level OS window —
    breaking the in-window scrim sheet (scrim painted in main window, dialog
    floated off, on Plasma Mobile spawning behind/off-screen = the exact "visible=1
    but user can't see it" symptom); PLUS a manual move() fighting the pos
    animation; PLUS redundant show()/raise()/activateWindow() racing it. FIX:
    stripped ALL of it — openEditor now just calls dlg->open() and trusts
    MobileDialog's sheet machinery (the same path every other MobileDialog in the
    app uses, which the user sees working elsewhere). (This is on TOP of last
    session's fix removing the exec()-era immediate deleteLater() that destroyed
    the dialog one event-loop pass after open().) (2) UI OLDER THAN REDESIGN —
    modernized. The editor used Theme COLOR tokens but HARDCODED radii (32px/6px)
    and font sizes (28/12/13/14/15px) inline. Replaced every hardcoded value with
    design tokens: avatar→FONT_2XL; status badge/banner/nick-label→FONT_SM;
    scId/notes/peer-name→FONT_BASE; nickname input→FONT_LG; all radii→RADIUS_SM
    (scId/copy/banner/inputs); all padding→SPACE_SM (SPACE_XS/2 for the tight scId
    chip). Verified: ZERO hardcoded font-size px remain; all 9 setStyleSheet
    blocks have balanced %N/.arg counts (parsed — 0 mismatches, so no silent
    QString placeholder mangling). ContactEditorDialog.cpp + ContactsPage.cpp
    COMPILE; full app builds+links ([89%] Built target SnapClone). DEVICE RETEST:
    the dialog should now slide up from the bottom as a sheet (scrim dimming the
    page behind) instead of being invisible; watch for the existing
    "MobileDialog::animateSheetIn settled" log line showing the dialog + scrim
    both visible. No more WindowStaysOnTopHint/topFlag in the openEditor log.

  - **★ CAMERA fullscreen redesign (Bug 7 full-bleed ambient preview) + per-frame
    PERF fix (frame coalescing), both compiled; coalescing math Rust-TESTED.**
    Two things from the camera redesign ask:
    (1) FULL-BLEED LAYOUT (CameraWidget.cpp/.hpp). Was topBar/viewfinder(stretch)/
    bottomBar in a QVBoxLayout — bars ate vertical space. Redesign: viewfinder
    fills the WHOLE widget, bars float as translucent overlay children of the
    viewfinder (same pattern the reticle/AF-badge/record-dot already use).
    Implemented WITHOUT breaking the classic layout: wrapped top+bottom bars in
    container QWidgets (m_topBarWidget/m_bottomBarWidget); new setFullBleed(bool)
    reparents them onto the viewfinder with a rgba(0,0,0,90) scrim + raises scene
    overlays above them, or restores them into the QVBoxLayout; layoutFloatingBars()
    pins them to top/bottom edges, called from the existing viewfinder-resize
    handler in eventFilter. Idempotent, gated (default OFF = classic). Compiles.
    DEFERRED: the actual toggle trigger (ambient-mode entry) + visual tuning —
    correctness is VISUAL, needs a screen, won't ship blind. setFullBleed is the
    mechanism, ready to wire.
    (2) ★ PER-FRAME PERF — frame coalescing (GstLaunchBackend.cpp). Found a real
    hitch: onStdoutData's `while(m_buf.size()>=sliceBytes)` emitted framePreview
    for EVERY buffered frame. fdsink (sync=false) writes frames at camera rate;
    if the GUI thread falls behind (heavy paint / stride re-inference / pause),
    N frames pile up and the loop decodes+copies (~3MB each) + runs the full
    rotate/mirror/scale/pixmap cascade for ALL N — but a live preview only wants
    the NEWEST. Worse, the wasted UI-thread work deepens the backlog (feedback
    loop → sustained lag). FIX: before the emit loop, if >1 whole frame buffered,
    fast-forward past the stale ones (drop bytes, no decode/emit), process only
    the newest; still advance recordFrameTime per skipped frame so the framerate
    health check sees the true rate; one-shot m_coalesceLogged debug line. The
    first-frame milestone + probe-mode path untouched. Compiles; full app builds.
    Modeled the decision as TESTED logic in snapclone-gstpipeline: coalesce_plan(
    buf_len, slice_bytes)→CoalescePlan{skip,process,remainder}. **6 tests**:
    process is always 0|1 (≤1 frame ever rendered); skip+process == whole-frame
    count (no frame lost/double-counted); remainder is exactly the sub-frame tail;
    zero-slice (geometry unknown) is safe. gstpipeline 22 tests. The C++ loop
    matches the model (whole=size/slice, skip=whole-1, process the rest).
    Threading note verified: QProcess::readyRead fires on the GUI thread (frames
    parsed+emitted there, no cross-thread queue pileup); applyUserRotation already
    early-outs at 0° (no per-frame copy in the common case); scale already picks
    Fast vs Smooth by downscale ratio. Coalescing was the missing piece.

  - **EDIT-CONTACT REGRESSION ("worked a couple builds ago, now shows nothing") —
    self-heal fix + pinpoint diagnostics.** User reports contact editing worked,
    then regressed. Traced the ENTIRE dialog subsystem looking for what changed:
    openEditor is still the clean dlg->open() (no WindowStaysOnTop creep-back);
    ContactEditorDialog compiles + all 9 stylesheet .arg chains balance (0
    mismatch — the font/radius modernization didn't break it); MobileDialog::open
    correctly takes the sheet path when m_isSheet (= isMobileFormFactor(), which
    has a robust logW<500 screen fallback so it's reliably true on FP5);
    repositionSheet + animateSheetIn geometry math is self-consistent; ALL
    dismissal paths (reject/accept/tap-outside-scrim) go through animateSheetOut →
    QDialog::reject()/accept() → finished(), which the openEditor finished-handler
    uses to clear m_openEditor + deleteLater; the accepted() save connection is
    intact. Code is sound for a normal open/close cycle — couldn't repro the
    regression statically + no git history in the snapshot to diff. PRIME SUSPECT:
    a STALE m_openEditor (QPointer<QDialog>) — if ANY close path hides the dialog
    without finished() reaching the handler (timing/ordering edge, direct hide(),
    animation interrupted), the stacked-editor guard at the top of openEditor sees
    a non-null pointer and "raises" a dialog that's already gone → every reopen
    shows NOTHING, permanently. FIX (self-heal, addresses the failure mode
    regardless of which path leaked): the guard now checks isVisible() — if the
    tracked dialog is genuinely visible, raise it (don't stack); if NOT visible,
    treat the pointer as stale, clear it + deleteLater the orphan, and fall
    through to open a FRESH editor. Plus a diagnostic: "openEditor: GUARD HIT —
    visible=N parent=..." and a self-heal log line, so the next device run
    confirms whether the stale-guard was the cause. ContactsPage.cpp COMPILES;
    full app builds+links. If the device log shows "GUARD HIT visible=0" then the
    stale guard WAS the regression and the self-heal resolves it; if editing still
    fails with no GUARD HIT line, the dialog isn't even reaching openEditor (a
    different, upstream cause — tap routing) and we chase that next.

  - **★ CAMERA PREVIEW: full-bleed redesign WIRED + activated, aspect-fill, gradient
    scrims. + a real preview-quality fix.** Continued the fullscreen camera work:
    (1) PREVIEW ASPECT FIX (real quality bug): onPreviewFrame scaled with
    Qt::KeepAspectRatio = LETTERBOX (black bars) when the 16:9 camera frame meets
    a different-aspect viewfinder. For a full-bleed preview that's wrong — you want
    edge-to-edge. Now aspect-aware: m_fullBleed → KeepAspectRatioByExpanding +
    centre-crop to the viewfinder rect (camera-app fill, same as the PIP overlay
    already does); classic mode keeps KeepAspectRatio (full frame, letterboxed).
    The crop is one bounded copy() per frame, only in full-bleed.
    (2) WIRED the redesign (was a dormant mechanism): CameraWidget ctor now reads
    QSettings [ui] fullBleedPreview (DEFAULT TRUE) and calls setFullBleed(true), so
    the redesign is actually live; set false in SnapClone.conf to roll back to the
    classic stacked layout, no rebuild. showEvent calls layoutFloatingBars() so the
    bars position correctly once the viewfinder has real geometry (ctor-time call
    couldn't — zero size then); viewfinder-resize eventFilter keeps them pinned.
    (3) GRADIENT SCRIMS: the floating bars were a flat rgba(0,0,0,90) which still
    washes white icons over a bright sky. Switched to qlineargradient scrims —
    darkest at the screen edge, transparent toward centre (top fades down, bottom
    fades up) — the camera-app convention: legible controls, undimmed frame centre.
    layoutFloatingBars pads bar height by SPACE_LG so the gradient has room to fade.
    (4) Confirmed the POST-CAPTURE preview is healthy: onFrameCaptured opens
    SnapComposerDialog (a MobileDialog sheet) which already shows the shot with
    KeepAspectRatioByExpanding. All syntax-checked; full app builds+links. DEFERRED
    (device-visual tuning, needs a screen): exact scrim opacity/height, whether the
    centre-crop chooses the right region for portrait vs landscape sensors, and the
    bar control spacing in full-bleed. setFullBleed default-on means the next build
    shows the redesign immediately for on-device judgement.

  - **CAMERA PREVIEW continued: tap-to-focus dead-zone fix + verified prior edits +
    log-scenario test.** Continued hardening the full-bleed redesign:
    (1) TAP-TO-FOCUS DEAD ZONE (real bug from the redesign): the floating bars are
    children of the viewfinder sitting ON TOP, and now have SPACE_LG of transparent
    gradient padding. A tap in that padding hit the bar widget (swallowed) instead
    of reaching the viewfinder's tap-to-focus event filter → dead zone for focus at
    the top/bottom screen edges. FIX: set WA_TransparentForMouseEvents on the bar
    CONTAINERS so taps on the visual-only gradient pass THROUGH to the viewfinder;
    the attribute doesn't propagate to children, so the actual control buttons
    still receive their taps. Restored (attribute=false) when returning to classic
    mode. Verified Qt semantics (visual paint unaffected, children unaffected).
    (2) Z-ORDER: confirmed layoutFloatingBars re-raises scene overlays (reticle/AF
    badge/landmark hint) AFTER the bars on every resize, so they stay on top of the
    gradient — landmark hint over the dark bottom gradient is actually MORE legible,
    and its tap still works since the bar is mouse-transparent. No collision.
    (3) VERIFIED the CameraRegistry USB-webcam diagnostic from 2 turns ago (flagged
    unverified) DOES compile; full app builds.
    (4) Added device_log_scenario_cold_fail_then_fresh_success test mirroring the
    FP5 log EXACTLY: cold start exhausts the full ladder (settle → relaxed → 4
    backoffs → swap), then a genuine fresh session begins at the TOP with a clean
    settle-retry — precisely the behavior observed when camera_29 finally worked
    ~14s later after the probe cycle. gstpipeline 26 tests; location 34; group 73;
    full app builds+links. DEFERRED (device-visual): scrim opacity/height tuning,
    portrait-sensor crop region, edge control spacing — all need a screen.

  - **★★ SNAP DELIVERY ROOT CAUSE FOUND + FIXED: same-WiFi hairpin, no LAN addr in
    CONNREQ.** Traced the latest log end-to-end. Sender queues snap → "No active
    session" → [CONNECT] fires → but connectByDhtId is a no-op ("lookup already in
    flight"). Later the PEER reaches US: "Connection request from sc-6eb25e40 @ TCP
    ws://69.156.66.68:9876" then "Skipping hairpin addr ws://69.156.66.68:9876" —
    and the cache shows "1 addr, 1 public, 0 same-LAN". ROOT CAUSE: BOTH phones are
    behind the SAME public IP (69.156.66.68 — same router/WiFi). The only address
    each had for the other was that shared public IP = a HAIRPIN, which we
    correctly skip — but with 0 same-LAN addresses there was then NOTHING left to
    try → connection never forms → snap stuck queued forever. WHY no LAN addr:
    d->selfAddr gets OVERWRITTEN with the public/reflexive IP once STUN resolves
    (AppController.cpp:1069), and the CONNREQ TCP/punch addrs are built from
    selfAddr → CONNREQ carried ONLY the public IP, never the 192.168.x.x. FIX
    (3 parts): (1) new d->lanAddr field preserves the LAN address across the STUN
    overwrite, set at first announce (primaryIp = LAN, pre-STUN). (2) Both CONNREQ
    producers (AppController_commands.cpp + AppController_connect.cpp) append the
    LAN addr as an OPTIONAL 6th field (CONNREQ\nfrom\ntcp\npunch\nts\nLAN) — only
    when it differs from the public IP. Backward-compatible: receiver uses
    parts.size()<5 (tolerant), so old peers ignore the 6th field, old senders omit
    it. (3) Receiver parses the 6th field and caches the LAN addr FIRST (preferred)
    so connectByDhtId tries the same-WiFi direct path before the hairpin. All 3
    files compile; full app builds+links. Modeled the decision as TESTED logic:
    snapclone-net-intel connreq_targets — order_targets(candidates, self_public_ip)
    drops hairpins, prefers LAN, dedupes; is_dead_end flags the stuck condition.
    6 tests incl the_bug_only_hairpin_public_is_dead_end + the_fix_lan_address_
    rescues_delivery (pins both the failure and the fix). net-intel 24 tests.
    ON-DEVICE: watch for "Connection request includes LAN addr 192.168.2.x —
    caching as preferred (same-WiFi direct path)" then a successful session — the
    snap should flush from the queue. If still stuck, the LAN addrs differ per
    phone subnet or AP client-isolation blocks peer-to-peer even on the LAN.

  - **AUDIT (network delivery path): verified ROBUST by design — connection-forming
    is the real unblock, not the delivery logic.** Deep-audited the full send path
    looking for why snaps don't reach receivers beyond the hairpin fix. Traced:
    sendToPeer queues on ensureSession(recipientId).pendingQueue + emits
    connectionNeeded (debounced); connectToPeer REUSES the same session (deleteLater
    on old transport, NOT reset — preserves pendingQueue + bumps epoch); on
    ITransport::connected the handler drains pendingQueue via transport->send (epoch-
    guarded against stale callbacks). Queue is NOT orphaned (same session key both
    sides). RELIABILITY MODEL is sound: every outbound snap is PERSISTED to the
    outbox FIRST (SnapRouter ~589, "always, even if peer offline"), tracked in
    m_v3PendingAck[peer][seq]=snapId; on piggybacked ACK → markSnapDelivered (sets
    delivered_at) + emit outboundDelivered (UI gets REAL delivery confirmation, not
    just "queued"); loadOutboxSnaps filters WHERE delivered_at='' so SYNC-on-
    reconnect only ever carries genuinely-undelivered snaps (no unbounded re-send,
    no re-delivery of confirmed). clearPeerState wipes the in-memory pending-ACK map
    on disconnect (Bug-5 path) — but that's just a fast-path cache; the snap SURVIVES
    in the outbox and re-delivers via SYNC hash-reconciliation when the peer
    reconnects. So a mid-flight disconnect does NOT lose the snap. CONCLUSION: the
    delivery pipeline (persist → send → ACK-confirm → SYNC-reconcile) is robust; the
    ONLY thing blocking delivery was the connection never forming (same-WiFi hairpin,
    fixed last turn by adding the LAN addr to CONNREQ). No code change this audit —
    documenting the path as VERIFIED-SOUND so future sessions don't re-chase it. The
    on-device test sequence to confirm: after the LAN-addr fix lets a session form,
    watch for "v3 ACK confirmed delivery of snap … (seq N <= ack M)" + outboundDelivered
    → that's end-to-end delivery proven.

  - **AUDIT (discovery + connect chain): verified SOUND end-to-end; "0 peers
    observed" is AP client-isolation, not a code bug.** Continued the delivery
    audit into discovery + session longevity. FINDINGS: (1) "NetworkManager stopped
    — all sessions closed" fires ONLY from ~NetworkManager (app shutdown), NOT on
    backgrounding — confirmed by the "=== SnapClone started ===" right after. So
    Background does NOT tear down the network; ruled out a "short delivery window"
    theory. (2) "session 0 ms, 0 peer(s) observed" = the two same-WiFi phones never
    saw each other. Gossip JOINs broadcast OUT to 192.168.2.255 + multicast, but no
    JOIN came IN. Audited GossipDiscovery: bind(AnyIPv4, port, ShareAddress|
    ReuseAddressHint) + joinMulticastGroup on every up IPv4 iface = CORRECT for
    receiving broadcast+multicast; onDatagramReady reads + processes fine ("Gossip
    packet received" is [D]-level so the user's rg-bound filter HID it — absence in
    log is inconclusive, not proof of non-receipt). processDatagram has a strong
    V32.35+ design: uses the datagram SOURCE IP as the authoritative LAN address
    (provably reachable) over the payload's self-reported host — exactly handles
    two-phones-one-WiFi; on new peer emits peerDiscovered (→connectToPeer) AND sends
    a reciprocal JOIN both multicast AND UNICAST to defeat multicast-blocking APs.
    The code is RESILIENT; "0 peers" is almost certainly AP CLIENT ISOLATION
    (router blocks client↔client bcast/mcast) — environmental. (3) Verified the
    last-turn hairpin fix's full chain: CONNREQ LAN addr → cached FIRST → 
    connectByDhtId loads cached addrs, same-/24 detection reorders LAN-first
    (AppController_commands.cpp ~98) → connectToPeer → pendingQueue flush / SYNC
    reconcile → ACK → outboundDelivered. EVERY link verified sound. CONCLUSION: the
    DHT-mailbox path (which does NOT need the AP to forward broadcast) is the
    correct + working fallback when gossip is isolation-blocked. No code change —
    documenting discovery+connect as VERIFIED-SOUND. On-device confirm: the new
    "Connection request includes LAN addr … same-WiFi direct path" + a session +
    "v3 ACK confirmed delivery" proves the whole chain. If gossip is wanted on an
    isolating AP, the only fix is router-side (disable AP isolation) — not code.

  - **★ RECEIVE-SIDE BUG FOUND + FIXED: silent media-write failure → permanently
    unviewable snap.** Audited the INBOUND path (transport→decrypt→store→inbox).
    Found a real bug in SnapRouter::handleV3Message F_DATA handling: writeMediaFile()
    returns bool but its result was IGNORED. On a failed disk write (no space /
    permission / bad path), writeMediaFile clears pathOut (localPath) to empty AND
    returns false — but the old code continued: set encryptedBlob (in-memory copy),
    then saveSnap() with an EMPTY localPath, then emit inboundReady. Consequence: a
    LIVE view works (SnapViewerDialog uses encryptedBlob from RAM) but after an app
    RESTART the blob is cleared to save RAM, localPath is empty, and the viewer
    shows "Media file not found" — the receiver effectively NEVER got the snap, with
    no error to the user. That's a real "snap didn't reach the receiver." FIX: check
    writeMediaFile's return; on failure, do NOT saveSnap/emit/ACK — log a warning
    ("media write FAILED; not storing/ACKing so SYNC will re-deliver. Check device
    storage.") and return early. CRUCIAL second half: the inbound frame carried
    F_ANNOUNCE, which had already inserted contentHash into m_peerKnownHashes[sender]
    (~line 831) — left there, our next SYNC would report the hash as KNOWN and the
    sender would SUPPRESS re-delivery → snap lost forever. So on failure we also
    m_peerKnownHashes[sender].remove(contentHash) (QSet<QString>::remove), making
    SYNC correctly report we DON'T have it → sender re-offers on next connection,
    giving the disk a chance to recover from a transient full/locked state. Net:
    a failed media write now self-heals via SYNC instead of silently losing the
    snap. Full app builds+links ([100%] Built target SnapClone). NOTE: env reset
    this session — had to reinstall cmake+qt6+cargo (per workflow notes: rm
    nodesource list first, apt-get install). On-device: a failed write now logs
    "media write FAILED" instead of a silently-broken inbox entry; the snap should
    re-appear after the next successful SYNC once storage frees up.

  - **★ CAMERA audit: QImage width/stride over-read guard + last-frame memory leak
    on stop.** Two camera findings:
    (1) OVER-READ GUARD (safety + tiling): in GstLaunchBackend the per-frame QImage
    is built with imgW from m_sinkWidth but imgStride from rowStride (forced →
    inferred → sink → tight w*4) — INDEPENDENT sources. If they disagree so that
    imgW*4 > imgStride, QImage reads imgW*4 bytes per row out of a buffer with only
    imgStride bytes/row = out-of-bounds read AND diagonal tiling. Added a clamp:
    imgW = min(imgW, imgStride/4), one-shot warning (m_strideWidthMismatchLogged)
    naming the inconsistency. Buffer is exactly sliceBytes = rowStride*frameH so
    imgStride*imgH fits — no under-read; the clamp only bounds the width
    interpretation. This hardens the long-standing tiling bug against the specific
    case where sink width and chosen stride are inconsistent.
    (2) LAST-FRAME MEMORY: m_lastFrame holds a full ~3.7MB frame (used by
    requestCapture→frameCaptured) and was NEVER released on stop() — a stopped/
    backgrounded camera kept a whole frame resident, wasteful on a memory-tight
    phone. Verified capture is SYNCHRONOUS (requestCapture emits frameCaptured
    inline, consumed before any later stop), so clearing on stop is safe. Now
    stop() does m_lastFrame = QImage() + m_buf.squeeze() to actually release
    capacity. AUDITED + RULED OUT: start()-after-start() does NOT leak m_proc (line
    493 no-op guard + line 496 kills/reaps/nulls the old proc before new-alloc);
    the framedrop timer leak was already handled. Full app builds+links ([100%]
    Built target SnapClone). (env reset this session — reinstalled cmake/qt6/cargo.)

  - **AUDIO + LANDMARK audit: characterization tests + a real latent compile-break
    fix.** Audited the three perception crates (audio/face/landmark).
    (1) AUDIO (snapclone-audio VAD): the biquad bandpass is a correct RBJ-cookbook
    constant-skirt design; the adaptive noise floor is well-built (holds during
    speech to resist ratchet-up, asymmetric rise/fall, clamped minimum). Found NO
    bug but it had thin coverage (8 tests, none on the adaptive floor). Added 5
    CHARACTERIZATION tests pinning the tuned behavior: floor adapts on quiet
    out-of-band noise; floor HELD (anti-ratchet) through sustained speech; floor
    never collapses below the clamp; attack faster than release; AND a deliberate
    test documenting a known tradeoff — a LOUD out-of-band frame scores high enough
    (energy-weighted) that the floor is held not adapted (surfaced when my first
    test assumed the opposite and failed — corrected to characterize reality, not
    my assumption). 13 audio tests, all pass. Did NOT alter the tuned DSP.
    (2) LANDMARK (snapclone-landmark): types.rs had ZERO tests. While adding them
    found a REAL LATENT COMPILE BREAK: `Landmark` derived Serialize/Deserialize but
    has a [u8;64] signature field — serde's built-in array impls stop at [T;32], so
    the derive does not compile wherever it's instantiated. It went unnoticed
    because the crate is rlib-only scaffolding NOT yet in the CMake/app build path,
    and Landmark is serialized EXCLUSIVELY via the manual byte codec in wire.rs
    (extend_from_slice), never via serde — so the derive was dead weight. FIX:
    removed the unused Serialize/Deserialize derive from Landmark (kept Debug,
    Clone); other types (LandmarkId/Scope/PhotoFingerprint, arrays ≤16B) keep their
    derives fine. Added 7 types.rs tests: Scope wire mapping (unknown bytes MUST map
    to Private so a corrupt/malicious payload can't accidentally publish — security
    relevant), Scope wire-value stability, fingerprint distance (self=0, symmetric,
    bounded 0..96, single-bit), is_none detection, AND a characterization test for a
    sharp edge — distance() does NOT special-case NONE, so comparing a real fp
    against NONE returns its popcount (could be small → spurious match); callers
    MUST guard with is_none(). Crate now compiles + 13 landmark tests pass (was 6).
    (3) FACE (snapclone-face, 40 tests): could NOT run this session — a transitive
    dep requires edition2024 / the workspace ed25519-dalek needs rustc≥1.81, but the
    apt toolchain is 1.75. Left unaudited rather than claim false coverage; it
    passed in prior sessions on a newer toolchain. NOTE: env reset — reinstalled
    cmake/qt6/cargo; the apt rustc (1.75) is older than some deps want, which is why
    face/landmark need version-pinning to run standalone here.

  - **DORMANT-CAPABILITY AUDIT + micro-DHT planning + Stage-0 store hardening.** User
    asked: find built-but-off features worth enabling (esp. encryption + DHT), maybe
    flip defaults, and think out loud about the micro-DHT. FINDINGS written up in
    docs/dormant-capabilities-and-microdht.md. Key facts:
    * SNAPCLONE_RUST_CRYPTO is OFF by default → the default build ships PLACEHOLDER
      crypto (HMAC-SHA256 XOR stream, "dev-only", fake key-agreement). The real crate
      (snapclone-crypto) has AES-256-GCM + X25519 ECDH + Ed25519 + ML-KEM/hybrid PQ
      and is sound, but its 9 tests ALL covered the ML-KEM path — the classical core
      (what an AES build uses) had ZERO tests. Added 6 (AES-GCM roundtrip/tamper/
      wrong-key/buffer-too-small, X25519 both-sides-agree, Ed25519 sign+verify+tamper)
      written against the real FFI sigs — but COULD NOT RUN them: apt rust 1.75 <
      edition2024 floor the modern crypto deps need. Must run on rustc≥1.81 before
      flipping. Also flagged: placeholder→X25519 key migration (existing identities
      invalidate) + cargo build-dep. RECOMMENDATION: flip crypto default ON with a
      one-time identity-regen migration.
    * DHT: default build has NO dht (LAN gossip/WS only). The device logs were from
      OPENDHT_BACKEND=ON builds. OpenDHT is heavy (gnutls/nettle/argon2, full Kademlia
      on a phone). RECOMMENDATION: keep OpenDHT opt-in; make the micro-DHT the default
      rendezvous once hardened.
    * MICRO-DHT (snapclone-microdht, 19→22 tests): a COMPLETE working system —
      node(UDP server + TTL sweeper) + store + line protocol(PING/PUT/GET/STATS) +
      client + standalone sc-microdht binary + C++ MicroDhtClient, gated behind
      beta/micro_dht_enabled=false. It's hub-and-spoke (one central node), which is
      exactly enough for the NAT-rendezvous job and far cheaper than Kademlia. PLAN
      (in the doc): Stage 0 harden the single node (sign PUTs w/ the existing Ed25519
      keys, rate-limit, value/TTL/size caps); Stage 1 replicate across 3-5 known nodes
      (PUT-to-all, GET-union — the store already returns a value LIST so union is
      natural) or rendezvous-hash the keyspace (consistent-hashing-lite, no finger
      tables); Stage 2 only go full Kademlia if it must be permissionless. Rationale:
      the micro-DHT exists BECAUSE OpenDHT is too heavy; re-implementing Kademlia
      badly throws that away. A replicated rendezvous with signed PUTs meets the real
      requirement in a few hundred lines, dependency-free, deterministically testable.
    * IMPLEMENTED Stage-0 first step: per-key value cap in store.rs
      (MAX_VALUES_PER_KEY=8, evicts soonest-to-expire on overflow, re-announce dedup
      doesn't count) — bounds memory so one client can't fill the node by PUTting
      unlimited distinct values under one key. +3 tests (cap enforced, keeps longest-
      lived, re-announce exempt). microdht 22 tests pass.
    NOTE: env reset again — apt rust 1.75 can run audio/gstpipeline/net-intel/microdht
    but NOT crypto/face/landmark-with-dalek (edition2024 / rustc≥1.81 floor).

  - **ROUTES + HOLE-PUNCHING analysis (carrier-pigeon/BLE first-class? cross-net punch
    on OpenDHT?).** Appended §5 to docs/dormant-capabilities-and-microdht.md. Findings:
    * The seam already exists: IDiscovery (NetworkManager.hpp) — start/stop,
      announcePresence, lookupPeer, peerDiscovered signal — implemented by BOTH
      GossipDiscovery and OpenDHT's DhtDiscovery. There's even a TransportDescriptor
      comment anticipating "LoRa, packet radio, sneakernet, mesh wifi". So "first-
      class route" = implement IDiscovery + register in the same resolver.
    * CARRIER PIGEON = PollinatorDiscovery, a LAN multicast beacon (224.0.0.251:9879,
      TTL=1) that ALREADY emits peerBeaconed(id,name,senderAddr) but is gated to only
      run while the Nearby screen is open. PROMOTE: run it always-on when foreground
      on a LAN, fold peerBeaconed into the normal peerDiscovered→connectToPeer path,
      and UNIFY with GossipDiscovery (they're two near-identical multicast beacons —
      wasteful). Cheap win for the isolation-free LAN case. Caveat: AP client-
      isolation blocks it too (same as gossip) — doesn't defeat isolation.
    * BLUETOOTH: honest finding — snapclone-proximity is RSSI ranging + bearing
      estimation ("point to find your friend"), NOT a data transport. No GATT/L2CAP/
      RFCOMM, no byte transfer anywhere. So BLE-as-route is a BUILD, not a flip. High-
      value framing: BLE as a SIGNALING transport (swap the small rendezvous blob —
      addresses+CONNREQ — over GATT), reusing proximity for presence. This is the ONE
      route that genuinely defeats AP client-isolation for co-located peers (BLE is
      peer-to-peer, no AP). BLE bulk transfer (~1-2Mbps) is painful for MB video snaps
      → signaling-first, bulk over WiFi.
    * HOLE-PUNCHING: STUN reflexive discovery + simultaneous UDP punch + TCP-after is
      NOT OpenDHT-specific (pure socket work). The only thing punching needs from "the
      DHT" is a SIGNALING channel: timely PUT/GET of {publicIP, punchPort} keyed by
      scID — which the micro-DHT already does. So cross-net punching is NOT inherently
      better on OpenDHT; OpenDHT only wins on (a) no central operator/SPOF and (b)
      zero-infra reachability via the public Jami network. Punch SUCCESS RATE is
      governed by NAT type (symmetric/CGNAT fail regardless) + relay fallback, not by
      which channel carried the endpoints. HIGHER-LEVERAGE WORK: wire up the empty
      network/relayUrl (TURN-style relay) — that, not OpenDHT-vs-micro-DHT, decides
      whether hard-NAT pairs ever connect. (Analysis only this turn — no code change.)

  - **BLE-as-control-plane analysis (mailbox / thumbnails / contact-sync, not photos).**
    Appended §6 to docs/dormant-capabilities-and-microdht.md. User's reframe is right:
    BLE's only weakness is bulk throughput, and every control/metadata payload is tiny.
    SIZE BUDGET (conservative ~20KB/s GATT goodput): mailbox blob ~107B (measured from
    logs: 24B nonce + 83B ct) = <10ms, SLAM DUNK; contact record ~80-150B each, 50
    contacts ≈5-7KB = <1s, easily fits; thumbnail ~5-10KB (160px JPEG q70) = 0.3-0.5s
    each, fits as BACKGROUND PREFETCH but marginal on-demand; full photo/video MB =
    out of scope (correct). BUILDING BLOCKS ALREADY EXIST: publishConnectionRequest is
    a keyed-blob PUT (transport-agnostic shape — BLE is just another carrier);
    VCardIO already serializes contacts to vCard 3.0 w/ routing ID + key + UID dedup
    (so BLE contact-sync = send vCard text → importVCard); thumbnails already cached as
    small JPEGs by ThumbnailService. DESIGN: one GATT "small blob" channel w/ a 1B type
    tag {MAILBOX,VCARD,THUMB} + 2B len; chunking only needed for thumbs (simple
    sequential, vs streaming MB media w/ flow control). KEY VALUE: works exactly where
    IP fails — AP client-isolation (BLE is peer-to-peer, no AP) and no-internet
    (festival/transit): swap mailbox blob → punch/connect over IP later; sync
    contacts/thumbs face-to-face; full media defers to outbox/SYNC when IP returns.
    Product story: near your friend → contacts+previews sync instantly over BLE → full
    snaps follow over WiFi. EFFORT: mailbox-over-BLE small, vcard-sync small, thumb-
    over-BLE medium (needs chunking + a thumb-hash diff mirroring snap SYNC). The one
    real new subsystem is the GATT service + identity-bonded pairing; payloads are all
    ready. RECOMMENDATION: this is a BETTER first BLE use than signaling-only or bulk
    media — build the small-blob channel, carry mailbox+vcard+thumbs, keep photos on
    IP. (Analysis only — no code change.)

  - **SCENARIO ANALYSIS: three-party multi-device meetup coordination.** User asked
    whether group conversations cover the "3 parties, multiple devices each, nearby but
    separated, one party at the meeting point" use case. Full assessment in
    docs/scenario-three-party-meetup.md. VERDICT: messaging substrate is strong; gaps
    are meetup-specific surface + multi-device identity. COVERS WELL: (1) group fan-out
    w/ sealed-sender + membership epochs that re-key on add/remove; (2) PARTITION
    TOLERANCE — dag.rs is a content-addressed Merkle DAG (snaps name parent ids → causal
    history; Lamport clock + deterministic tiebreak → every device computes the SAME
    total order on reconverge — exactly the "separated then back together" case); (3)
    catch-up via mailbox gaps() (per-sender seq gap detection → pull only missing); (4)
    Wayfinder shows peers by bearing+log-distance. GAPS: (A) ⚠️ MULTI-DEVICE NOT MODELED
    at group layer — group::Member is {sc_id, x25519_pub}, ONE key per person, so
    "multiple devices each" = duplicate members + messages sealed to wrong device. Note
    the micro-DHT store ALREADY anticipates multi-device ("multiple values per key") but
    the group membership layer doesn't. Fix: person-key + device-sub-keys, seal to the
    person, link devices over BLE/QR. (B) ⚠️ MEETING POINT not a first-class object —
    location is peer-centric (Wayfinder = people relative to you), no shared DESTINATION.
    Fix: a "Pin" group-snap type (lat/lon+label) that rides existing group fan-out/DAG/
    catch-up + a Wayfinder DESTINATION mode (bearing/dist to pin + each member's dist to
    pin) reusing existing geometry; party-at-point drops pin at their location = "come
    to me". (C) no sub-party/who-is-with-whom clustering (flat roster) — cluster co-
    located members using proximity+location. (D) no per-member delivery indicator in
    groups ("2 of 3 parties have seen this") — pure UI synthesis over existing per-sender
    seqs+ACKs. RANKED: 1) shared Pin + Wayfinder destination mode (highest value, reuses
    everything), 2) multi-device device-sets (the correctness gap), 3) per-member
    delivery indicator, 4) co-location clustering. All ride existing infra (DAG/mailbox/
    ratchet/Wayfinder/proximity/BLE) — no new transport, no re-arch. Caveat: group has 73
    passing tests but multi-device is untested (model doesn't exist yet). Analysis only.

### V32.37 — log-spam + shutdown segfault (device-log confirmed, built + runtime-verified)

Two bugs confirmed by FP5 device log (2026-05-30 09:45):

**1. CameraRegistry log flood.** The "Unmatched pipewire node.name(s)" diagnostic
(src/ui/camera/CameraRegistry.cpp) fired on every availableDevices() call — ~30×/sec —
emitting the same long node-name list each time. The pw-dump SUBPROCESS is already cached
500ms (kPwDumpCacheMs), but the cheap re-parse (and this log line) repeats per call.
FIX: log-on-CHANGE only. Sort the unmatched names, join as a key, keep it in a function-local
`static QString s_lastUnmatchedLogged`, and qCInfo only when the key differs from last logged.
Names still surface once (and re-surface if hardware actually changes), spam gone. No change to
the pw-dump cache itself.

**2. Shutdown segfault (use-after-free).** Crash was `segmentation fault (core dumped)` right
after the "Network stats summary at shutdown" line. ROOT CAUSE: AppController::Impl members
destruct in REVERSE declaration order; `network` (NetworkManager) is declared FIRST so it
destructs LAST — after router/session/storage/crypto are already gone. NetworkManager::stop()
(run from its dtor) closes transports, which can emit peerDisconnected and deleteLater()
transports, firing slots that dereference the already-destroyed d->router / d->storage /
d->crypto = use-after-free.
FIX: replaced `AppController::~AppController() = default;` (src/core/AppController.cpp) with an
explicit dtor that tears down network + discovery FIRST, while those subsystems are still alive:
disconnect() network/jami/pollinator (sever Qt signals), then reset() jami, pollinator, network
in that order. Remaining members then auto-destruct cleanly.

VERIFIED: full app built clean ([100%] Built target SnapClone). Headless offscreen run +
SIGTERM → exit 143 (clean), no 139 segfault, no core dump after the stats summary.

Decisions reaffirmed: diagnostics in hot loops must log-on-change; members touching
cross-subsystem Qt signals must be torn down explicitly in the dtor before reverse-order
auto-destruction.

### V32.38 — full-bleed camera controls: dead taps + hard gradient fades (device-confirmed)

Device photos (10:22) of the redesigned full-bleed camera page showed: (1) controls
visible (back ←, "…then choose", "Front", reload ↻) but TAPS DO NOTHING; (2) gradient
fades with a hard edge / no feathering.

**Root cause (dead taps).** V32.37 full-bleed (setFullBleed in src/ui/CameraWidget.cpp)
floated the top/bottom bars over the viewfinder and set the whole BAR
WA_TransparentForMouseEvents, with a comment claiming the attribute "does NOT propagate to
children." That's false for a child widget: Qt hit-tests the mouse-transparent bar as
see-through across its ENTIRE rect — including where its buttons sit — and forwards the
press to the viewfinder behind it. The buttons never get hit-tested → visible but dead.

**Fix.** Split the scrim from the bar:
- New m_topScrim / m_bottomScrim QWidgets (lazily via ensureTopScrim/ensureBottomScrim,
  parented to the viewfinder). The scrim carries the gradient and is the ONLY
  WA_TransparentForMouseEvents piece, sitting lowered behind the bar. Its extra
  gradient-pad height never makes a dead zone — taps there fall through to the viewfinder
  (tap-to-focus preserved).
- The BAR is now interactive (WA_TransparentForMouseEvents=false) with a transparent
  background, sized to its controls only and anchored to the edge, raised above the scrim.
  Buttons receive their taps again.

**Fix (hard fades).** The scrim gradients were 2-stop linear ramps (read as a band). Added
intermediate eased stops (top: 150→80→24→0 alpha at 0/.35/.7/1; bottom mirrored
0→24→90→170) so the fade feathers smoothly with no visible edge.

layoutFloatingBars() now positions scrim (bar height + SPACE_LG pad) behind the
controls-only bar; classic-mode path hides scrims and restores interactive bars.

VERIFIED: full app built clean ([100%] Built target SnapClone). Header/impl consistent
(scrim members + helpers present; bar mouse-transparency now false in both paths).

FOLLOW-UP (not done — awaiting design sign-off, per user): bottom menu ("CAMERA" bar +
page-dot strip) to adopt the new full-bleed scrim look once the design is confirmed.

Decision: a mouse-transparent container does effectively swallow taps meant for its
children; keep the gradient on a separate mouse-transparent scrim layer and the controls
on an interactive (background-transparent) bar above it.

### V32.38b — full-bleed scrim becomes frosted glass with subtle chromatic shift

Follow-up to V32.38 (per user: glass should refract the LIVE viewfinder, shift starts
subtle, must run well on BOTH Fairphone 5 and desktop).

New class Camera::GlassScrim (include/ui/camera/GlassScrim.hpp, src/ui/camera/GlassScrim.cpp;
registered in src/ui/CMakeLists.txt source + AUTOMOC header lists). Replaces the plain
QWidget scrims. It does NOT sample the screen (QWidget can't read sibling pixels cheaply;
QGraphicsEffect won't blur a live sibling). Instead CameraWidget feeds it the strip of the
current viewfinder pixmap under its geometry — those pixels are already produced every frame
(the `scaled` viewfinder QImage in onPreviewFrame), so zero extra capture cost.

Per frame, GlassScrim::rebuildGlass():
  1. downscale the strip by 4 (the downscale IS most of the blur AND drops every later op to
     ~1/16 the pixels — this is what keeps it affordable on FP5),
  2. 1px separable box blur on the tiny image (frost),
  3. subtle chromatic shift: take R from a left-shifted copy, B from a right-shifted copy, G
     from centre (shift = 1 final px by default = faint fringe; setChromaticShift() tunable),
  4. upscale back (bilinear = free extra softening),
  5. paint via a DestinationIn gradient alpha mask (eased multi-stop, edge→transparent, top
     fades down / bottom fades up) so it only shows near the edge, then a faint dark tint on
     top for control legibility.

Mouse-transparent (taps fall through to viewfinder); interactive control bar sits above it —
so the V32.38 dead-tap fix is preserved. Flat stylesheet gradients removed from setFullBleed
(GlassScrim paints its own; a flat gradient on top would re-introduce the hard band).
CameraWidget::updateScrimBackdrops(scaled) called right after setPixmap; no-op when not
full-bleed. Scrim member type now Camera::GlassScrim*; forward-declared.

Cost note: blur/shift run on a width/4 × ~30px image ~30fps — trivial on both targets. If FP5
ever shows strain, the lever is m_downscale (raise to 6-8) or throttle backdrop feed to every
2nd frame.

VERIFIED: configure + build clean (GlassScrim.cpp.o compiled; [100%] Built target SnapClone);
headless offscreen run + SIGTERM → exit 143 clean, no scrim asserts/errors.

CAVEAT (honest): this is a per-channel PIXEL OFFSET on a blurred backdrop — a real, visible
chromatic-aberration fringe, but not physically-refractive lensing. True edge-refraction
(displacement that bends with a normal map) needs the GLSL path in GstLaunchBackend GPU
Stages B-D (deferred). Current approach is the cheap one that runs everywhere.

Decision: frosted glass over live content = feed the scrim the already-computed viewfinder
strip + downscale-blur-shift, rather than backdrop-sampling effects (which don't see live
siblings) or full-res per-frame blur (too costly on FP5).

### V32.38c — full-bleed dead taps: REAL root cause (QLabel parent clips child events) + settings micro-DHT copy

**Dead taps, actually fixed this time.** Clue from device: Tab activates the controls,
mouse/touch does not. Tab traversal ignores geometry/stacking; mouse needs the button to be
the topmost widget at the point. So the buttons were enabled + in the focus chain but not
receiving mouse events. Added a temporary runtime probe (childAt at each control's center) —
under the previous design EVERY control returned childAt=nullptr.

ROOT CAUSE: full-bleed reparented the bars to m_viewfinderLabel — a QLabel. A QLabel clips
child MOUSE-EVENT delivery to its content rect; children still paint, and Tab still reaches
them (geometry-independent), but a press isn't delivered to a child the label considers
outside its hit region. (The earlier V32.38 WA_TransparentForMouseEvents fix was real but
not the whole story; the QLabel parent was the live blocker.)

FIX: parent the bars AND the GlassScrims to `this` (the CameraWidget), not the label. In
full-bleed the viewfinder fills the whole widget, so this-coords == viewfinder-coords and all
the geometry math (layoutFloatingBars, updateScrimBackdrops) is unchanged — but `this` is a
plain QWidget that delivers events to children normally. Post-fix probe: backButton/
cameraSelector childAt resolves to the button itself (the controls inside the degenerate
offscreen rect; the rest were offscreen-window artifacts). Temp probe removed after confirming.
Classic (non-full-bleed) path already parented to `this`, unaffected.

VERIFIED: build clean ([100%] Built target SnapClone); temp diag removed (0 refs); headless
run + SIGTERM exit 143 clean.

**Settings: micro-DHT description updated.** The Beta-testers blurb (src/ui/SettingsPage.cpp
buildBetaTestersSection) said only "run a throwaway DHT … exercise publish/lookup." Rewrote
it to describe what micro-DHT actually does now: a small hub-and-spoke rendezvous node
(sc-microdht) speaking PING/PUT/GET/STATS that stores the opaque, already-encrypted records
the app uses to find peers — reachable address + connection-request mailbox — with a bounded
number of values per key (oldest-expiring evicted first) and per-record TTLs; node never sees
plaintext; no node-to-node routing yet (which is what makes it good for deterministic offline
testing). Header comment above the function updated to match.

Decision: interactive overlays floated over the viewfinder must be children of the
CameraWidget, NOT the QLabel viewfinder — a QLabel clips child mouse-event delivery to its
content rect (dead clicks, but Tab still works — a useful signature for this class of bug).

### V32.38d — full-bleed gesture audit: bar-gap dead zones, mobile touch routing, dead label, surprise-flip

Audit of the full-bleed camera design for tap/gesture confusion or blockage, after the
V32.38c reparent-to-`this` fix. Findings + fixes:

**F1 (regression from my own fix) — bar-gap dead zones.** The top/bottom bars span full
width and are mouse-OPAQUE (must be: empirically verified that WA_TransparentForMouseEvents
on a parent swallows its children's clicks even when the child sets the attribute false — so
"transparent bar + opaque buttons" is impossible). Controls only fill part of the width; the
stretch gaps look like preview but are bar pixels, so taps there were dead (no focus). FIX:
event filter on the bars forwards presses that miss a control to focus.

**F2 — mobile touch routing for bar children.** To catch gap taps the bars now accept touch
(WA_AcceptTouchEvents). Per the existing ShutterButton note, an ancestor that accepts touch
can suppress Qt's mouse-synthesis for a child that doesn't — so a FINGER tap on the back/
rotate/combo could be lost (worked with a mouse, dead on Plasma Mobile). FIX: when a TouchBegin
lands on an interactive child, synthesize the mouse press/release to that child (the proven
SwipeNavigator forwardTap pattern) and consume the touch so Qt can't double-activate.

**F3 — dead label spot.** The "Shoot then choose recipient" QLabel sits mid-top-bar; childAt
returns it, and deferring to it (non-interactive) made it a dead spot. FIX: only defer to
genuinely interactive children (QAbstractButton/QComboBox/focusable); forward taps on
non-interactive children (labels) to focus like the gaps.

**F4 — surprise camera flip from the chrome strip.** Gap taps route to focus; routing them
through handleViewfinderTap would mean a double-tap near the top bar flips the camera
(surprising). FIX: new triggerFocusOnly(point) — focus sweep with NO double-tap-flip arming;
bar gaps/labels use it. The viewfinder body keeps full single-focus/double-flip behavior.

**Checked-OK (no change):** ShutterButton already accepts touch directly to bypass swiper
interception (tap=photo/hold=record/swipe-up=lock) — correct, same hazard class handled.
SwipeNavigator.forwardTap walks to the first interactive ancestor and posts synthetic mouse
events — buttons (QPushButton/QComboBox) are recognized, so page-swipe vs control-tap
discrimination on the camera page is sound. Reticle/AF badge/record dot/PIP are all
WA_TransparentForMouseEvents (don't intercept).

VERIFIED: build clean ([100%] Built target SnapClone); headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY (can't fully exercise touch routing headlessly): on Plasma Mobile confirm (a)
finger taps on back/rotate/camera-combo activate them; (b) finger tap in a top/bottom bar gap
runs focus (reticle appears); (c) no double-activation of bar buttons; (d) double-tap on the
preview body still flips, double-tap on the bar strip does NOT.

Decision: full-width chrome bars over a live preview must forward non-control presses to
focus (gaps/labels = preview-like), defer only to interactive children, synthesize touch→mouse
for those children when the bar accepts touch, and keep double-tap-flip off the chrome strip.

### V32.38e — glass icon buttons: dark mottled frosted fill + press scale + tap-point ripple

Per user: bring back button press animation and use a darker, more-opaque scattered/mottled
frosted-glass fill on the chrome buttons. Chosen direction: darker glass + soft radial
mottling; press anim = scale-down + spring-back PLUS a ripple expanding from the actual tap
point.

New class Camera::GlassIconButton (QPushButton subclass; include/ui/camera/GlassIconButton.hpp,
src/ui/camera/GlassIconButton.cpp; registered in src/ui/CMakeLists.txt source + AUTOMOC).
Painted (not QSS) because the look needs things stylesheets can't do:
  * dark opaque base rgba(12,11,10,235) — deeper/more opaque than the old flat
    rgba(36,34,32,0.88) #iconBtn fill;
  * soft radial MOTTLING — a faint white highlight lobe (upper-left) + a dark deepening lobe
    (lower-right), two off-centre QRadialGradients at low alpha, so the disc reads as uneven
    frosted glass rather than flat;
  * thin 1px ring for definition (brighter on hover);
  * glyph (text()) drawn centered.
Animation via two Q_PROPERTYs:
  * pressScale (1.0→0.88 OutQuad on press; →1.0 OutBack spring on release), painted as a
    scale about the center;
  * ripple (0→1 OutCubic on release) — a radial lightening keyed to the press coordinate
    (m_rippleOrigin from mousePressEvent), which a QSS :pressed state can't know.
Durations from Theme (ANIM_FAST / ANIM_NORMAL+120).

Wired: m_backButton + m_rotateButton now GlassIconButton (both setFixedSize TOUCH_ICON=47 so
the discs match; objectName stays "iconBtn" so QSS still supplies font-size/size hints; the
opaque paint covers the QSS background). Still QAbstractButton → the V32.38d touch-forwarding
filter still treats them as interactive children.

VERIFIED: configure + build clean (GlassIconButton.cpp.o compiled; [100%] Built target
SnapClone); RENDERED idle + pressed states to PNG and visually inspected — dark mottled disc,
ring, glyph, and the pressed frame shows scale-down + ripple lightening as intended; headless
run + SIGTERM exit 143 clean.

NOTE: only the back/rotate icon buttons use this. The shutter (Video::ShutterButton) and the
camera combo are unchanged. If you want the same glass treatment on the bottom-bar controls,
that's the natural next step once the bottom menu adopts the new look (still parked).

Decision: chrome buttons over the live preview are painted (dark mottled glass) not QSS-filled,
so they can carry tap-point ripple + scale animation and a non-flat frosted texture.

### V32.38f — bottom bar: camera-page-only glass appearance (transition), unchanged elsewhere

Per user: the shared bottom bar should TRANSITION to the new glass appearance ONLY on the
camera page and stay exactly as-is on every other page.

Key fact: the bottom strip in the device photo (‹ CAMERA › + page-position dots) is the
app-wide UnifiedNavBar — ONE widget created by MainWindow, parented to the swiper wrapper,
shared by all pages. So "camera-only" means the single bar must paint differently when the
camera page is current, without affecting the others.

Added to UnifiedNavBar:
  * setCameraGlassMode(bool) — toggles a target flag and animates a Q_PROPERTY glassAmount
    (0=normal, 1=glass) over Theme::ANIM_NORMAL. On turn-ON it immediately switches the
    stylesheet background to transparent (so nothing opaque flashes); on turn-OFF it restores
    the opaque BG_RAISED stylesheet only AFTER the fade-out finishes (animation finished slot).
  * paintEvent now, when glassAmount>0, fills a dark vertical gradient (top edge ~70α →
    mid 140α → base rgba(8,7,6,225), all scaled by glassAmount) BEFORE the dots — matching the
    top/bottom viewfinder scrims. On every other page glassAmount==0 so the block no-ops and
    the bar is visually identical to before.

Driven from MainWindow: connect(swiper, pageChanged, …setCameraGlassMode(idx==kPageCamera)),
plus an initial call for the starting page. pageChanged (not pageSettled) so the bar
crossfades as the camera page lands (the "transition" the user asked for).

VERIFIED: build clean ([100%] Built target SnapClone); RENDERED the glass gradient at
amount=1 over a bright backdrop and inspected — light-at-top → dark base, "CAMERA" + accent
dot legible, matches the scrims; headless run + SIGTERM exit 143 clean.

NOTE: this restyles the bar BACKGROUND on the camera page. The segment/label/dot painting is
unchanged. If you also want the nav buttons themselves to get the GlassIconButton treatment on
the camera page, that's a further step. Other pages are provably untouched (amount 0 = no-op).

Decision: a shared chrome widget gets page-specific appearance via an animated paint flag
driven on pageChanged, NOT by swapping widgets per page — keeps one bar, one layout, and makes
"only on page X" a paint-time conditional that cleanly no-ops elsewhere.

### V32.38g — gst camera audit: R/B-swap misclassification (color) + stuck bloat latch after dim change (tiling)

Continued GStreamer camera audit for color + tiling. Two real defects found + fixed.

**COLOR — R/B swap auto-detect misclassified pad/alpha-first formats.** In
GstLaunchBackend.cpp the sink-format → swap decision was:
    swapRB = upper.startsWith("RGB") || upper.startsWith("ARGB");
Worked out the actual byte layouts (GStreamer names = memory byte order; Qt Format_RGB32 LE =
bytes [B,G,R,x], so the decoder assumes R@byte2, B@byte0):
  * BGRx/BGRA → R@2 B@0 → NO swap (correct as-is)
  * RGBx/RGBA → R@0 B@2 → R/B swap (rgbSwapped fixes)
  * xRGB/ARGB/xBGR/ABGR → pad/alpha byte FIRST → components ROTATED not mirrored → a simple
    R/B swap CANNOT fix them.
The old heuristic (a) missed xRGB/xBGR entirely (start with 'x' → no swap → tint), and (b)
WRONGLY set swapRB for ARGB (rgbSwapped leaves ARGB still wrong). FIX: replaced with an
explicit per-format classification — NoSwap {BGRx,BGRA}, SwapRb {RGBx,RGBA}, and a
pad/alpha-first set that is NOT silently "fixed": it logs a loud qCWarning that colors may be
wrong and the real fix is forcing BGRx negotiation. Pipeline always negotiates BGRx so this is
the defensive fallback path; the point is it no longer gives false confidence.

Extracted the decision into the tested authority crate: snapclone-videosrc gains
`PixelOrder { NoSwap, SwapRb, NotCheapFixable }` + `classify_pixel_order(&str) -> Option<..>`
(None for YUV/unknown). +4 tests (bgr→noswap, rgb→swap, alpha-first→not-cheap-fixable incl the
ARGB regression guard, unknown→None incl the old "string" parser-bug value). videosrc now 38
tests, all green (apt rustc 1.75, zero-dep). C++ classification mirrors the crate exactly.

**TILING — bloat/stride-inference latch not reset on mid-stream dimension change.** The padded-
stride inference is one-shot per pipeline (gated by m_bufBloatLogged, reset only at start). But
the sink-caps parser can RE-SYNC m_frameWidth/Height mid-stream when GStreamer negotiates dims
different from requested (it clears m_buf + recomputes m_frameBytes). If frame data had already
tripped the bloat path BEFORE that caps line was parsed, the latch stayed set and
m_inferredStride held a value computed for the OLD dims → padded stride for the NEW size was
never detected → tiling/diagonal-drift persisted for the whole session. FIX: the dim re-sync
now also resets m_inferredStride=0 and m_bufBloatLogged=false so the new layout gets a fresh
inference pass.

VERIFIED: videosrc 38 tests pass; full app build clean ([100%] Built target SnapClone); fixes
present (padFirst classification ×3, buggy startsWith("ARGB") gone); headless run + SIGTERM
exit 143 clean.

DEVICE-VERIFY: (a) colors correct on the FP5 preview (no red/blue tint) — log line "Sink
format: … noSwap/doSwap/padFirst"; (b) if a pad/alpha-first format ever negotiates, the new
qCWarning fires (and that's the signal to fix videoconvert caps); (c) if preview dims ever
re-sync mid-stream ("SINK dims differ from requested"), tiling now self-corrects because the
stride is re-inferred for the new size.

Decision: pixel-format → R/B handling is enumerated + tested in snapclone-videosrc, not an
ad-hoc startsWith; pad/alpha-first formats are surfaced loudly rather than mis-"fixed". Any
mid-stream layout change must reset BOTH the inferred stride and the one-shot bloat latch.

### V32.38h — gst camera audit (cont): coalesce frame-time corruption + buffer-cap re-misaligns frames (tiling)

Continued the camera bug hunt across the frame-buffer/coalescing paths. Two real bugs.

**TILING (med) — runaway-buffer cap cut at the WRONG size + off a frame boundary.** End of
onStdoutData, the safety cap was:
    if (m_buf.size() > m_frameBytes*4) m_buf = m_buf.right(m_frameBytes*2);
m_frameBytes is the TIGHT size (w*h*4); the real frame is sliceBytes (padded rowStride ×
height). With a padded stride this (a) kept too little and (b) cut m_buf at an offset that is
almost never a frame boundary — so the next m_buf.left(sliceBytes) sliced ACROSS a frame
boundary → diagonal tearing/tiling, defeating the stride work. FIX: express the cap in
sliceBytes, keep 2 whole frames, and re-align to a frame boundary (drop the leading partial).
Verified the boundary math standalone: for assorted oversized buffers it always yields exactly
2.000 whole frames.

**HEALTH METRIC (low) — coalescing injected zero-interval frame timestamps.** The fast-forward
that drops stale frames called recordFrameTime(now) once PER skipped frame — all with the same
"now". That puts N-1 zero deltas into m_frameTimes, dragging the MEDIAN inter-frame interval
toward 0 (apparent ∞ fps) and MASKING a genuine framerate slowdown (the opposite of the
comment's intent of "see the true capture rate"). FIX: distribute the skipped frames' synthetic
timestamps EVENLY across the interval since the last real sample (last + span*i/steps), so the
recorded cadence reflects the true average production rate; and when span<=0 (first-ever batch,
no cadence info) skip them entirely rather than invent zero-deltas.

Also AUDITED-OK this pass (no change needed): the QImage build reads exactly sliceBytes
(== imgStride*imgH) so no over-read; the imgW clamp prevents per-row over-read; stop() tears
down the watchdog BEFORE killing the proc, disconnects signals, nulls-first, reparents before
delete (no use-after-free / "Destroyed while running"); the framedrop watchdog handler guards
on (!m_active || m_currentNode != node); the negotiation retry ladder (settle→relaxed→backoff→
alternate) is bounded at every stage (no infinite loop); recordFrameTime caps m_frameTimes at
30.

VERIFIED: full app build clean ([100%] Built target SnapClone); headless run + SIGTERM exit
143 clean; cap-alignment math checked standalone (always whole frames).

DEVICE-VERIFY: with a padded-stride sensor under GUI load, preview should NOT tile after a
buffer-cap event; framerate-degraded warning should still fire when the sensor genuinely drops
rate even if frames were coalesced.

Decision: every buffer-size computation in the receive path must use sliceBytes (real padded
frame), never the tight m_frameBytes, and any buffer trim must re-align to a frame boundary.
Synthetic frame timestamps must never be coincident (zero-delta) — distribute or omit.

### V32.38i — DESIGN (no code): unified messaging dashboard daemon + shared DBus backend

User wants a unified messaging dashboard (online contacts, photo thumbnails, unread) for both
a Plasma applet (kicker/QML) and a Qt-widget panel, sharing ONE DBus backend, as a TRUE
background daemon (works while the GUI app is closed). Wrote full design to
docs/dashboard-daemon-and-dbus.md. Analysis only — multi-week refactor, fixing the shape
before any code.

Feasibility findings (better than expected):
- AppController is a QObject (NOT QWidget; only Qt include is <QObject>) and owns the whole P2P
  core (network/crypto/storage/router/session/discovery) with zero widget deps → already
  structurally daemon-able. GUI depends on controller, not vice-versa.
- main.cpp already builds controller separately from MainWindow → clean split seam.
- App already links Qt6::DBus AND already speaks DBus (PowerManager → org.kde.Solid.
  PowerManagement) → exposing a service is an extension, not a new dep.
- SingleInstanceGuard already runs a QLocalServer (IPC precedent).
- Presence is already live/event-driven (peerConnected/Disconnected + peerDiscovered → markOnline).
- ONLY GUI coupling in core: AppContext reads QGuiApplication::applicationState() for power
  decisions → becomes a headless conditional, not a blocker.
- THE catch: ThumbnailService is in-memory QPixmap, can't cross processes → must add a
  QImage-based headless variant that writes small JPEGs to a cache dir; DBus passes PATHS not
  bytes (dovetails with the BLE small-blob thumbnail spec).

Design highlights:
- Topology: snapclone-daemon (QCoreApplication) is the SINGLE owner of the transport+DB+keys;
  GUI and both dashboard surfaces are DBus clients of org.snapclone.Dashboard (session bus).
- DBus contract drafted: Contacts() -> a(ssbi) id/name/online/unread; RecentThumbnails(path-
  based); PresenceChanged/UnreadChanged/ThumbnailReady signals; InterfaceVersion for KDE
  cadence skew.
- 6 stages, each independently shippable: (0) extract QImage pixel logic; (1) daemon skeleton;
  (2) DBus service; (3) single-owner cutover [HIGH risk — the real work]; (4) thumbnails+unread;
  (5) Qt-widget panel then Plasmoid. First SOUND shippable dashboard is after Stage 3 (two core
  owners before that = DB race + split presence). Fallback: GUI-app-as-service (skips 1+3,
  loses "works app-closed").
- SECURITY (must-resolve): session bus = any process as the user. So session-bus exposure
  opt-in default-OFF; thumbnail cache opt-in default-OFF + 0700 + prune + exclude ephemeral/
  view-once; NO message bodies over DBus (presence/counts/paths only); daemon key-at-rest model
  TBD (unattended load vs GUI-handed session key vs KWallet).

OPEN QUESTIONS for user (in doc §7): key-at-rest model; may ephemeral snaps be thumbnailed
(reco: no); phone-only or desktop Plasma too; ship widget panel before Plasmoid?

Decision: pursue headless daemon (Option B) — core is already widget-free QObject owning the
transport, so the cost is the single-owner cutover, not the headless core. Thumbnails path-
based via daemon-written cache. All session-bus exposure + on-disk previews opt-in/default-off
(E2E app, session bus is a wider trust boundary).

### V32.38j — camera audit (cont): VCM fd data race between camera-open and focus sweep

Continued the camera bug hunt into the focus/VCM concurrency. Found a real data race.

BUG (concurrency, can corrupt focus state / double-drive the lens motor): the camera-open path
(CameraWidget, m_vcmOpenTimer lambda) set `m_focusInProgress = true` with a PLAIN store, then
launched a QtConcurrent worker that opens/rebinds the VCM fd. But a CDAF focus sweep
(runFastFocusSweep) drives the SAME VcmController fd from ITS OWN worker thread and acquires
m_focusInProgress via compare_exchange_strong. If a sweep was already in flight, the open path's
plain store was a no-op on the (already-true) flag, yet it STILL launched its own worker on the
same fd — so two threads issued VCM ioctls concurrently, and whichever finished first cleared
m_focusInProgress while the other was still running on the fd (also defeating the teardown wait
in onSelectorChanged, which relies on that flag to know when focus work is done).

FIX: the open path now acquires the flag ATOMICALLY via
compare_exchange_strong(expected=false, true). It launches the open worker only if it WINS the
flag; if a sweep already owns it, the open is skipped (the VCM is opened lazily on first use
anyway) and we just refresh the AF badge. Now both VCM-touching workers (open + sweep) gate on
the same atomic acquire, so only one ever drives the fd at a time. Removed an orphaned
watcher->setFuture(future) left from the old structure.

Also AUDITED-OK this pass (no change): requestCapture emits m_lastFrame which is a deep
.copy() (no buffer aliasing with the reused QByteArray); the GUI-thread load()-then-
invokeMethod focus paths (triggerFocusOnly, pendingFocusTimer, onSceneChangeTick) are safe
because the real acquire is the compare_exchange inside runFastFocusSweep — the load() checks
are just early-outs to avoid queuing redundant invocations; the async sweep's raw m_vcm pointer
is protected by the bounded teardown wait on m_focusInProgress.

VERIFIED: build clean ([100%] Built target SnapClone); both flag-acquire sites now use
compare_exchange (grep count 2); headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY: rapidly switch camera (triggers open) while a tap-to-focus sweep is mid-run —
should no longer get stuck "AF …" state or erratic focus; only one VCM worker runs at a time.

Decision: every code path that drives the VcmController fd from a worker thread must acquire
m_focusInProgress via compare_exchange_strong (atomic test-and-set), never a plain store —
plain stores allow two workers onto one fd and corrupt the in-progress signal teardown relies
on.

### V32.38k — camera preview HiDPI: viewfinder + PIP rendered at logical res (soft/blurry on scaled displays)

Continued the preview visual-bug hunt into the DISPLAY side (scale/crop/blit). Found a real
sharpness bug on HiDPI.

BUG (visible quality): onPreviewFrame scaled the frame to m_viewfinderLabel->size() — which is
LOGICAL pixels — and showed QPixmap::fromImage(...) with an implicit devicePixelRatio of 1. On
a scaled display (Plasma Mobile / FP5 commonly run >1x, often fractional) the label is
size×dpr DEVICE px, so a logical-size pixmap is painted at logical resolution and upscaled by
the compositor → a soft/blurry preview that also throws away available sensor detail. There was
ZERO devicePixelRatio handling anywhere in the camera display path, and no app-wide high-DPI
attribute. The PIP overlay had the same bug (scaled to its logical size).

FIX (3 spots, all dpr-aware; dpr==1 collapses to prior behaviour exactly so no change on
unscaled displays):
  * Main viewfinder: compute dpr = label->devicePixelRatioF(); scale + centre-crop in DEVICE
    px (logical × dpr); pm.setDevicePixelRatio(dpr) so the label presents device px at logical
    size, crisp. Verified the mechanism standalone (a 720px pixmap @dpr2 reports 360px logical).
  * updateScrimBackdrops: `scaled` is now DEVICE px but scrim geometry() is LOGICAL — map each
    scrim rect by ×dpr before slicing so the frosted-glass strips sample the right region (the
    GlassScrim downscales internally, so a device-px strip is fine/higher quality).
  * PIP overlay: same scale-to-device-px + setDevicePixelRatio.
Side note: the 17/10 FastTransformation threshold now compares against device px, so HiDPI
gets SmoothTransformation more often — correct (more target pixels to fill well).

AUDITED-OK this pass: rotate-then-mirror order for PIP (comment correctly notes order matters
for 90/270°); applyUserRotation uses SmoothTransformation once per frame; classic (non-full-
bleed) path uses KeepAspectRatio (letterbox) vs full-bleed KeepAspectRatioByExpanding (crop) —
both intentional.

VERIFIED: build clean ([100%] Built target SnapClone); all 3 dpr fixes present; headless run +
SIGTERM exit 143 clean. dpr mechanism confirmed via standalone test.

DEVICE-VERIFY: on the FP5 (scaled display) the live preview + PIP should be visibly sharper
(native-res, not compositor-upscaled); no change expected on a 1x desktop.

Decision: any camera pixmap shown in a QLabel must be scaled in DEVICE pixels (logical × dpr)
and tagged with setDevicePixelRatio(dpr); logical-size pixmaps are soft on scaled displays.

### V32.38l — camera preview visual: stale placeholder style on switch + REC indicator hidden behind top bar

Continued preview visual-bug hunt via STATE TRANSITIONS + OVERLAY OCCLUSION. Two real bugs.

**BUG 1 (every camera switch) — placeholder style not cleared.** On switch, the viewfinder
shows "Switching camera…" with a stylesheet `background:#1a1a1a; padding:20px`. When the first
frame of the new camera arrives, onPreviewFrame calls setPixmap() — which does NOT clear the
label's text or stylesheet. So the live frame is painted INSIDE 20px padding over the dark bg →
a shrunken, dark-bordered preview after every switch until something else resets the label.
FIX: in onPreviewFrame, when the label still carries placeholder text, reset text="" and
restore the baseline `background:#000000;` BEFORE setPixmap. Guarded on text non-empty so the
steady-state path doesn't eat a per-frame style recompute.

**BUG 2 (full-bleed recording) — REC indicator occluded by the top bar.** m_recordIndicator is
a child of the viewfinder at (12,12). After the V32.38c reparent, the bars are children of
`this`; the viewfinder (and its children) stacks BELOW them. In full-bleed the top bar covers
y=0..barH, so the "● REC 0:00" badge at y=12 is HIDDEN behind the chrome — and raise() can't
fix it (it only reorders within the viewfinder's own children, a different parent). FIX: new
positionTopOverlays() pushes the REC indicator below the top bar's height in full-bleed
(restores to the top inset in classic). Called from layoutFloatingBars AND at the live-REC show
site (recording can start after the last layout pass).

AUDITED-OK this pass: m_mirrorPreview is set (from CameraRegistry metadata) BEFORE backend->
start() on switch, so the new camera's first frame is mirrored correctly (no one-frame
wrong-mirror flash); the focus reticle is a viewfinder child positioned in viewfinder-local
coords matching the tap point (incl the V32.38d bar-gap forward path) — lands correctly, dpr-
independent since move() is logical; mirror-then-rotate vs rotate-then-mirror order is correct
for PIP.

VERIFIED: build clean ([100%] Built target SnapClone); headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY: (1) after tapping switch-camera, the new preview fills the frame with NO dark
border / shrink; (2) recording in full-bleed shows the ● REC badge BELOW the top chrome, not
hidden behind it.

Decision: setPixmap never clears a QLabel's text/stylesheet — any placeholder state set on the
viewfinder must be explicitly reset when the first real frame arrives. Overlays that must sit
above the floating chrome cannot rely on raise() if they live under a different parent; position
them clear of the bar instead (or reparent).

### V32.38m — camera preview visual: stale mis-sized frame on resize / rotation / keyboard

Continued preview visual-bug hunt via RESIZE / ORIENTATION-CHANGE. Found a real bug.

BUG: there is no resizeEvent override; resize is handled in the m_viewfinderLabel eventFilter
(QEvent::Resize), which repositioned PIP/bars/AF-badge but did NOT rescale the displayed frame.
The viewfinder QLabel is NOT setScaledContents, so on resize it keeps the OLD-size pixmap
(AlignCenter) until the next live frame overwrites it. So on window resize / device rotation
(portrait↔landscape on FP5) / on-screen-keyboard show-hide, the preview briefly shows wrong —
full-bleed: black gaps or off-centre mis-crop; classic: wrong letterbox — and shows wrong
INDEFINITELY if frames are paused (app backgrounded) or the stream stalled.

FIX: extracted the scale/crop/dpr/blit logic from onPreviewFrame into a shared
renderToViewfinder(displayImg) helper (so the live path and resize path can't drift). The
Resize branch now re-renders the cached m_latestFrame (mutex-read, mirror re-applied) through
that same path, so the preview tracks size changes immediately even while paused/stalled.
Ordering: reposition bars/scrims (layoutFloatingBars) BEFORE the re-render so
renderToViewfinder's scrim-backdrop sampling uses the NEW scrim geometry, not stale.

AUDITED-OK this pass: AF badge/landmark-hint/PIP already reposition on the same Resize event;
m_latestFrame is the post-rotation frame so re-render needs only mirror re-applied (rotation
already baked in); the helper extraction preserves the exact live-path behaviour (same
FastTransform threshold, full-bleed crop, dpr tag, placeholder reset).

VERIFIED: build clean ([100%] Built target SnapClone); renderToViewfinder used by both live
(onPreviewFrame) and resize paths; headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY: rotate the phone or toggle the OSK while previewing — the image should re-fit
the new viewfinder size IMMEDIATELY with no black gaps / wrong letterbox, and should re-fit
even when the preview is paused (e.g. backgrounded then resized).

Decision: any QLabel-based viewfinder that isn't setScaledContents must re-render the cached
frame on resize (a non-scaled QLabel keeps a stale-size pixmap otherwise); share one render
path between the live and resize routes so their scale/crop/dpr logic can't diverge.

### V32.38k — camera audit (cont): VCM rebind leaves driver unbound on failure + CDAF baseline mismatch

Continued the camera bug hunt into VcmController + the CDAF sweep.

**BUG 1 (VcmController::rebindAfterStreamStart — leaves hardware worse-off).** The dw9719
rebind dance was: close() → write unbind → sleep → write bind → sleep → open(). If the BIND
write failed after the UNBIND succeeded, it `return false`'d, leaving the VCM driver DETACHED
entirely — strictly worse than before (no AF AND no subdev) until something rebinds it. (This
runs on the QtConcurrent worker from the open path I hardened in V32.38j, so a failure here is
silent.) FIX: after a successful unbind, RETRY the bind up to 3× (200ms apart) before giving
up; on total failure, log loudly that the driver is left detached and AF needs a manual
rebind/reboot. Unbind-failed (driver state unchanged) still returns early as before.

**BUG 2 (FocusSweep runCdafSweep — finalSharpness didn't match the parked position).** After
parabolic sub-step refinement moved the lens to `finalPos`, the function still reported
`finalSharp` from the PRE-refinement discrete sample. finalSharpness feeds m_focusBaseline (the
scene-change re-focus threshold), so a too-low baseline makes the detector trigger-happy. FIX:
after moving to the refined position (we're already settled there), measure one more settled
frame and adopt its sharpness (guarded: only if >0, and max() so we never report worse than the
sampled best). NOTE: this is the LONG/rarely-used sweep path; the PRIMARY runtime path
runFastFocus() does NOT have this bug — it parks at the discrete best and reports that exact
sample's sharpness (position and sharpness already consistent). Fixed runCdafSweep for
correctness/consistency regardless.

Also AUDITED-OK this pass (no change): VcmController fd lifecycle (open guards double-open,
close nulls fd, setPosition clamps to [min,max], setPositionNormalized clamps 0..1);
laplacianSharpness is stride-aware (bytesPerLine), ROI-bounded with neighbour margins, n==0
guarded, correct variance formula, width/height<6 guard; coarse/fine sweep position math is
bounds-clamped (fineMin/Max via std::max/min); parabolic denom has 1e-9 epsilon guard + result
clamped to [minPos,maxPos]; runFastFocus has proper cancellation checks before each move + final
park.

VERIFIED: build clean ([100%] Built target SnapClone); headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY: (1) if dw9719 bind transiently fails during rebind, AF retries rather than
leaving the lens driver detached (watch for the new "rebind FAILED to re-bind" warning only on
genuine repeated failure); (2) long-sweep CDAF baseline now reflects the refined position (no
spurious re-focus right after a long sweep).

Decision: an unbind must always be followed by a best-effort rebind (never leave hardware
detached on a transient sysfs write failure); any reported focus sharpness must correspond to
the lens position actually parked at, since it becomes the scene-change baseline.

### V32.38n — camera preview audit: stale cached frame survives camera switch

Continued the camera-preview bug hunt (rotation/mirror/scale/cache paths).

**BUG (preview correctness + focus hazard): m_latestFrame not cleared on camera switch.**
onSelectorChanged (single-device path) stops the old backend, clears the VISIBLE pixmap, and
shows "Switching camera…", but leaves the CACHED frame m_latestFrame holding the PREVIOUS
camera's last frame. m_latestFrame is read by (a) the resize/resume re-render (eventFilter
Resize, V32.38m) and (b) the CDAF focus sweep (getter at ~3317, also ~3099/3294/3392).
Consequences: a resize/rotation/keyboard-toggle DURING a switch re-blits the OLD camera's image;
worse, a focus sweep racing the switch could evaluate sharpness on the stale frame and drive the
NEW camera's VCM to a wrong lens position. FIX: clear m_latestFrame = QImage() under
m_frameMutex right where the viewfinder is cleared during the switch (a focus worker may read it
concurrently; consumers already isNull()-check).

Also AUDITED-OK this pass (no change — important to record so these aren't re-flagged):
- Front-camera mirror asymmetry is INTENTIONAL + documented: the live preview is mirrored
  (m_mirrorPreview, display-only) so selfie motions feel natural, but the captured photo, the
  recorded .mkv, the QR-scan frame, and previewFrameAvailable all use the UN-mirrored `oriented`
  (so scene text reads correctly to the recipient). NOT a bug.
- PIP mirror is correct: PIP is always the front camera (startPip invariant), so the
  unconditional mirrored(true,false) there is right, and rotate-then-mirror order is correct
  (matches the documented 90/270° concern).
- applyUserRotation: validates m_userRotation ∈ {0,90,180,270}, QTransform rotate, smooth.
- renderToViewfinder HiDPI: scales/crops in DEVICE px (logical×dpr), tags pixmap dpr → crisp on
  scaled displays; updateScrimBackdrops correctly maps logical scrim geometry into device-px
  before slicing (V32.38k).
- Resize re-render (V32.38m) already re-blits the cached frame at the new size with mirror +
  bar reposition — fixes the "stale-sized pixmap until next frame / indefinitely if paused" case.
- Big-downscale uses FastTransformation (≥1.7×), SmoothTransformation otherwise — sensible.

FOLLOW-UP (not changed, lower risk): the PIP-mode switch early-return paths (PIP fallback +
PIP start) don't clear m_latestFrame either; the main focus sweep keys off the main viewfinder
so the risk is smaller, but for invariant simplicity a future pass could clear on every switch
path. Flagged, not done.

VERIFIED: build clean ([100%] Built target SnapClone); headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY: switch cameras then immediately rotate the device / toggle keyboard — should NOT
briefly show the previous camera's frame; tap-to-focus right as you switch should not mis-drive
the new lens.

Decision: any cached frame that feeds re-render or focus MUST be invalidated when the source
camera changes — clearing the visible pixmap alone is insufficient because the cache has other
readers.

### V32.38o — camera preview audit: recording-mode preview bypassed the display pipeline

Continued the camera-preview bug hunt into the placeholder/text state machine + the
recording-mode preview path.

**BUG (preview consistency): recording-mode preview poll bypassed renderToViewfinder.** During
video recording the live backend is busy, so the viewfinder polls a JPEG the recorder pipeline
writes and displays it. That path did an INLINE setPixmap with display.scaled(label->size(),
KeepAspectRatio, Smooth) — bypassing the shared renderToViewfinder helper and therefore missing
three things it does: (1) the V32.38l placeholder text/style RESET — so a leftover "Starting
camera…"/error stylesheet (padding:20px + colored background) kept its padding and colored
border AROUND the polled recording frame; (2) DPR-correct scaling — soft on HiDPI; (3) full-
bleed expand+centre-crop — letterboxed instead of filling during recording. FIX: route the
polled frame (post-mirror; rotation already applied upstream by the recorder's videoflip, and
renderToViewfinder does not rotate, so no double-rotation) through renderToViewfinder, matching
the live path exactly.

**BUG (same class): RECORDING fallback placeholder didn't reset prior style.** When no preview
JPEG exists yet, a "RECORDING" placeholder pixmap is painted at vfSize and setPixmap'd directly.
A leftover error/"Starting…" stylesheet's padding:20px would inset it with a stray colored
border. FIX: reset text+style to plain "background:#000000" before showing the placeholder
(guarded on text/style non-empty).

Also AUDITED-OK this pass: onBackendError surfaces the error in the viewfinder and does NOT
auto-clear (next successful frame's setPixmap overwrites it) — intentional + correct; it also
releases the CameraHealthService in-use marker so probes aren't suppressed forever after an
error. The live renderToViewfinder placeholder reset (text non-empty → reset) is correct for
its per-frame hot path (cheap check, only resets on the transition out of a placeholder).

VERIFIED: build clean ([100%] Built target SnapClone); headless run + SIGTERM exit 143 clean.

DEVICE-VERIFY: start recording right after a camera error or during "Starting camera…" — the
recording preview should fill edge-to-edge with NO colored border/padding and be crisp on the
FP5's scaled display (not soft/letterboxed).

Decision: ALL viewfinder frame display must go through renderToViewfinder (placeholder reset +
DPR + full-bleed crop); any path that paints its own viewfinder pixmap must first clear leftover
placeholder text/style or a prior stylesheet's padding/background bleeds around the image.

### V32.38p — camera diagnostics harness + unitized frame gate (from device log analysis)

User still seeing camera issues; asked for a hardware-validatable testing harness with
unitized camera logic to debug iteratively. Analyzed the pasted FP5 log (2026-06-02 09:43):
pipeline is actually HEALTHY there (probes Working ttff 353/429ms; live 1280x720; every
"Preview frame layout OK" shows tight stride bpl 5120 = 1280×4, bytes 3686400 — no
tiling/color issue in THIS log; double-tap flip rear→front works). TWO real findings:

1. **Misleading log: "frames-seen: 30" was m_frameTimes.size()** — the 30-entry fps RING
   BUFFER, capped at 30, logged as if it were total delivery. Both a 5.5s and an 11s session
   reported exactly 30. FIX: added m_sessionFrameCount (real per-session counter, reset at
   start, ++ per delivered frame); stop() now logs `frames-seen: N (fps-window: M)`.
2. **Preview pauses on transient app-inactive** — log shows repeated "Frame processing PAUSED
   (visible: true, appActive: false)". shouldProcessFrames() hard-gates on m_appActive, which
   flips immediately on applicationStateChanged with NO debounce. On desktop/Plasma, active
   flips false for transient reasons (notifications/focus) → preview freezes though fully
   visible. NOT yet changed in the app (product call — see harness verdict first).

HARNESS (new):
- `snapclone-videosrc` gains the gate as PURE TESTED logic: FrameGate, GateReason,
  frame_gate() (exact mirror of shouldProcessFrames precedence: app-inactive → screen-off →
  scan-mode → visible), and frame_gate_debounced(ms_since_active, grace_ms) modelling the fix
  (ride through a brief inactive blip if visible+screen-on; still pause immediately on
  screen-off/offscreen). +9 tests → videosrc now 47 tests, all green (stock cargo).
- New binary `sc-camdiag` (rust/snapclone-videosrc/src/bin/sc_camdiag.rs, pure std): runs the
  EXACT app gst pipeline, reads raw frames off fd, reports REAL frames/fps/max-gap/stalls/
  bad-layout (via validate_preview_frame)/freeze-heuristic + a VERDICT (HEALTHY / LOW FPS /
  STARVED / NO FRAMES / LAYOUT ERRORS / POSSIBLE FREEZE). Builds with stock rustc, runs
  headless on the FP5. Usage + interpretation in docs/camera-diagnostics.md.

The harness separates "is the PIPELINE healthy?" from "is the APP's gating/display wrong?" —
if sc-camdiag says HEALTHY but the app preview freezes, it's the gate (wire the debounce); if
it says LOW FPS/STARVED, it's the pipeline/driver.

VERIFIED: videosrc 47 tests pass; sc-camdiag binary builds; full app builds clean ([100%]
Built target SnapClone).

NEXT (await user): run sc-camdiag on the FP5 during a repro and paste the report; if HEALTHY +
preview still freezes on focus-loss, wire frame_gate_debounced (≈400ms grace) into
shouldProcessFrames/updateProcessingState.

Decision: debug camera issues via an on-device harness that runs the real pipeline + unitized
tested decision logic, rather than guess-fixing blind. Misleading diagnostics (counter that
caps at the ring-buffer size) must report the real quantity.

### V32.38q — upnp build blocker fixed + sc-camdiag duplicate-detection hardened (device signal triaged)

Two items from on-device testing.

**BUILD BLOCKER (fixed): snapclone-upnp stale re-export.** lib.rs did
`pub use mapper::{PortMapper, MappingResult}` but mapper.rs had been refactored — the
PortMapper struct/state-machine became a free async fn `open_mapping()`. E0432 unresolved
import broke the whole rust build. FIX: re-export `{open_mapping, MappingResult, LEASE_SECS,
RENEW_SECS}` to match what mapper.rs actually exports. Confirmed: snapclone-upnp now compiles
(only harmless unused-import warnings). (Only doc-comment mentions of "PortMapper" remain.)

**CAMERA SIGNAL from sc-camdiag (real, asymmetric):**
- REAR camera_29: delivery 46.2 fps, but 282/451 frames flagged byte-identical → VERDICT
  POSSIBLE FREEZE. 46fps is itself anomalous (front does clean 30).
- FRONT camera_10: 30.1 fps, 0 duplicates → HEALTHY.
So the rear and front behave differently; the rear appears to emit ~46fps with most frames
repeated (effective new content ≈17fps = choppy preview), OR my 8-byte freeze heuristic was
fooled by `videoscale add-borders=true` letterbox bars (static black region → sampled bytes
identical every frame) if the rear sensor's native aspect ≠ 16:9.

**HARNESS HARDENED to disambiguate:** replaced the 8-byte sample with a FULL-FRAME FNV-1a hash
(immune to border-sampling false positives), and the report now distinguishes:
- `delivery fps` (raw) vs `CONTENT fps (unique)` (the rate that actually matters for smoothness),
- `duplicate frames N (X% of delivered)`,
- a one-shot letterbox probe (top row uniform but mid row not → add-borders is padding this
  sensor → aspect mismatch), appended to the CONTENT-fps line,
- new verdicts: DUPLICATE-PADDED (emits Nfps but only Mfps new — source repeating frames) vs
  LOW CONTENT FPS vs HEALTHY. Builds + 47 lib tests pass.

NEXT (await user re-run): run the updated sc-camdiag on the REAR camera and paste. Interpretation:
- If CONTENT fps ≈ delivery fps (no border note) → the earlier "freeze" was the 8-byte artifact;
  rear is fine, look elsewhere.
- If CONTENT fps ≈17 and "DUPLICATE-PADDED" → the rear source really repeats frames; likely
  fix is to CAP the source framerate in the gst pipeline (add framerate=30/1 to the negotiated
  caps) so pipewiresrc/libcamera stop emitting duplicate-padded frames. (Not applied yet —
  confirm with the harness first; framerate caps can also cause negotiation failures on some
  sensors, so this needs the data.)
- If the letterbox note fires → the rear preview is being letterboxed by add-borders (aspect
  mismatch), a separate display issue.

VERIFIED: snapclone-upnp compiles; sc-camdiag rebuilt; videosrc 47 tests pass.

Decision: make the duplicate/freeze signal trustworthy (full-frame hash, content-fps,
letterbox probe) before acting; the 46fps-with-duplicates pattern is most likely source-side
frame repetition, addressable via a source framerate cap — but only after the hardened harness
confirms, since framerate caps risk negotiation failure on some sensors.

### V32.38r — preview freeze ROOT CAUSE found + fixed: app-inactive gate (pipeline ruled out)

sc-camdiag (hardened, full-frame hash) on BOTH FP5 cameras, repeated across two phones:
- REAR camera_29: 46.2 fps delivery == 46.2 fps CONTENT, 0 duplicates, no border note → HEALTHY
- FRONT camera_10: 30.1 fps == 30.1 CONTENT, 0 duplicates → HEALTHY
CONCLUSION: the capture pipeline is NOT the problem. The earlier "282 identical / POSSIBLE
FREEZE" was a false positive in the old 8-byte sampling heuristic (samples landed in a static
scene region); the full-frame FNV hash shows every frame unique. Layout clean, no stalls, no
tiling, no color issue on either sensor. (Rear genuinely runs ~46fps, front ~30fps — both fine.)

ROOT CAUSE of the reported "camera issues" is therefore APP-SIDE, and the evidence points to the
gate: device logs show "Frame processing PAUSED (visible: true, appActive: false)". Confirmed by
reading the code:
- applicationStateChanged→appActive=false only sets the flag + logs; it does NOT stop the
  backend (only SCREEN-OFF stops the pipeline).
- So on focus loss the backend keeps delivering frames (~30-46fps, proven) but onPreviewFrame
  early-returns on !shouldProcessFrames() and DROPS them → the preview FREEZES on the last frame
  while the app keeps capturing. On desktop/Plasma, ApplicationActive flips false for transient
  reasons (notifications, focus), so the viewfinder stutters/freezes on every blip.

FIX (wired the previously-modelled debounce): shouldProcessFrames() now mirrors the tested
snapclone_videosrc::frame_gate_debounced — when app-inactive but still visible + screen-on, keep
processing for kInactiveGraceMs (400ms) before pausing. applicationStateChanged records
m_inactiveSinceMs and arms a one-shot grace timer to fire the PAUSED transition if the blip
lasts. Genuine backgrounding (grace expires) and screen-off / offscreen still pause exactly as
before; on the phone, real backgrounding keeps app_active false past the grace → pauses (battery
preserved). The C++ gate is a line-for-line mirror of the Rust model that has 9 passing tests.

VERIFIED: full app build clean ([100%] Built target SnapClone); headless run + SIGTERM exit 143
clean; gate members/logic present. Rust model: 47 tests pass (frame_gate + debounce).

DEVICE-VERIFY: on the camera page, trigger a transient focus loss (notification / click a panel /
brief overlay) — the preview should NOT freeze for sub-400ms blips; fully backgrounding the app
should still pause (and screen-off still powers down the pipeline). Watch the log: PAUSED should
no longer fire for momentary appActive=false flips.

Decision: the harness EARNED its keep — it conclusively ruled out the pipeline and redirected the
fix to the real culprit (the gate), and the hardened duplicate detection prevented a wrong fix
(needless framerate cap) from the earlier false positive. Wire tested Rust models into the C++
gate rather than hand-reimplementing.
