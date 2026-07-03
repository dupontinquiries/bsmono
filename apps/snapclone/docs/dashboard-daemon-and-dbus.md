# Unified messaging dashboard: background daemon + shared DBus backend

Status: **design only, no code yet.** Target chosen by user: a *true background
daemon* (presence works while the GUI app is closed), exposing the *full
dashboard* (online contacts, photo thumbnails, unread counts), consumed by
*both* a Plasma applet (kicker/QML) and a Qt-widget panel over *one* DBus
backend.

This is a multi-week refactor. The point of this doc is to fix the shape, the
interface contract, the security model, and a staged build order BEFORE writing
code, because the "works app-closed" requirement forces the hard part (a headless
core that owns the encrypted transport) and that decision ripples everywhere.

---

## 1. Feasibility — better than it looks

Findings from the current tree that make this tractable:

- **`AppController` is a `QObject`, not a `QWidget`.** Its only Qt include is
  `<QObject>`. It owns the entire P2P core — `NetworkManager`, `ICryptoService`,
  `StorageManager`, `SnapRouter`, `SessionManager`, `IDiscovery`,
  `NatTraversal`, `PollinatorDiscovery`, etc. — with no widget dependencies. The
  GUI depends on the controller, not the other way round. So the core is already
  structurally daemon-able.
- **`main.cpp` already constructs the controller separately from the window**
  (controller at line ~261, `MainWindow` at ~283). Clean seam to split on.
- **The app already links `Qt6::DBus` and already SPEAKS DBus** — `PowerManager`
  calls `org.kde.Solid.PowerManagement`. Exposing a service is an extension of an
  existing capability, not a new dependency.
- **`SingleInstanceGuard` already runs a `QLocalServer`** — there's a precedent
  for an IPC endpoint and single-owner semantics.
- **Presence is already live + event-driven** — `AppController` bridges
  `NetworkManager::peerConnected/peerDisconnected` and
  `IDiscovery::peerDiscovered` into `markOnline`. That signal stream is exactly
  what the dashboard subscribes to.

The ONE GUI coupling in the core: `AppContext` reads
`QGuiApplication::applicationState()` for foreground/background power decisions
(written defensively via `qobject_cast`). In a daemon there is no "app active"
state — this becomes a compile/run conditional, not a blocker.

The ONE thing that is genuinely in-memory-only and needs rework for cross-process
sharing: **thumbnails.** `ThumbnailService` produces in-memory `QPixmap`s, not
files. You cannot hand a `QPixmap` to a separate process cheaply (see §4).

---

## 2. Process topology

```
            ┌──────────────────────────────────────────────┐
            │  snapclone-daemon  (QCoreApplication)          │
            │  OWNS: AppController core — network, crypto,    │
            │        storage, router, session, discovery      │
            │  - the ONLY holder of the encrypted transport   │
            │  - the ONLY writer of the snap DB + key store   │
            │  exposes:  org.snapclone.Dashboard (session bus)│
            │  activation: systemd user service + DBus        │
            │              activation; stays resident         │
            └───────▲───────────────────▲────────────────────┘
                    │ DBus              │ DBus
        ┌───────────┴──────┐   ┌────────┴───────────────┐
        │  GUI app (Qt)    │   │  Dashboard surfaces      │
        │  becomes a CLIENT │   │  - Plasmoid (QML)         │
        │  of the daemon    │   │  - Qt-widget panel        │
        │  (camera, compose,│   │  both read the SAME        │
        │   full UI)        │   │  org.snapclone.Dashboard   │
        └──────────────────┘   └───────────────────────────┘
```

Key rule: **exactly one process owns the P2P socket, the SQLite DB, and the key
store.** Today that is the GUI app. After the split it is the daemon, and the GUI
becomes a client like any other. Two processes both opening the encrypted
transport / DB would be a correctness and security disaster (double connections,
DB write races, split presence). This single-owner constraint is the whole reason
the refactor is non-trivial.

### Who owns the keys when the GUI is closed?
This is THE policy decision for Option B. The daemon must be able to decrypt
incoming snaps to generate thumbnails/unread while the GUI is closed, which means
the daemon holds (or can load) the private key unattended. Options, to decide:
- (a) Daemon loads keys from the existing key store at `AppDataLocation`
  unattended (simplest; key is as protected as the file system / disk encryption).
- (b) Keys unlocked only when the GUI has run once this session and handed the
  daemon a derived session key (presence works closed, but new-content decryption
  pauses until first unlock).
- (c) KWallet integration for the at-rest key.
Recommendation: start with (a) on the phone (single-user, disk-encrypted), design
the interface so (b)/(c) can slot in without changing the DBus contract.

---

## 3. DBus interface contract — `org.snapclone.Dashboard`

Bus: **session bus** (per-user). Object path `/org/snapclone/Dashboard`.

> ⚠️ Privacy boundary (see §6): anything on the session bus is readable by any
> process running as the user. Contact names + thumbnails of received private
> snaps are sensitive. The contract below is the *capability*; §6 governs what is
> safe to actually expose and how to gate it.

### Properties (cheap, cached, change-signalled)
- `b Online` — daemon-to-network connectivity (not per-contact).
- `i UnreadTotal` — sum of unread across conversations.

### Methods
- `a(ssbi) Contacts()` → array of `(id, displayName, online, unreadCount)`.
  Presence + unread in one round-trip; the common dashboard query.
- `a(sxs) RecentThumbnails(i maxCount)` →
  `(snapId, timestampMsUtc, thumbnailPath)`. Returns **file paths**, not bytes
  (see §4). Path points at a small JPEG the daemon wrote to a cache dir the
  client can read.
- `(sxs) LatestThumbnailFor(s contactId)` → one most-recent preview for a
  contact (the per-row dashboard avatar/preview).
- `MarkRead(s conversationId)` — let the dashboard clear unread (optional; may be
  GUI-only initially).

### Signals
- `PresenceChanged(s id, b online)` — fired off the existing
  peerConnected/Disconnected bridge.
- `UnreadChanged(s conversationId, i count)`.
- `ThumbnailReady(s snapId, s thumbnailPath)` — new preview available.

### Versioning
Include a `u InterfaceVersion` property. The Plasmoid and widget panel must
tolerate a daemon that predates a given method (KDE ships on its own cadence).

---

## 4. Thumbnails across processes — the real catch

`ThumbnailService` today returns in-memory `QPixmap`s keyed by `(path,size)`. A
separate process can't receive a `QPixmap`. Two viable designs:

- **(A) Path-based (recommended).** Daemon writes small JPEG thumbnails (e.g.
  128×128, the size the dashboard needs) to a dedicated cache dir under
  `QStandardPaths::CacheLocation/snapclone/thumbs/`, named by content hash. DBus
  passes the *path*; the client loads it directly. Cheap, no marshalling of
  bytes, and it dovetails with the BLE small-blob thumbnail work already specced
  in `docs/` (thumbnails ~5–10KB). Lifecycle: daemon prunes the dir (LRU/size
  cap) since it's the owner.
- **(B) Bytes over DBus.** Marshal the JPEG bytes inline. Fine for a *handful* of
  ~5–10KB thumbs; gets ugly for a scrolling history. Avoid as the primary path.

Either way the thumbnails must be generated by the **daemon**, not the GUI, since
the daemon is what's running when a snap arrives app-closed. That means the
crop/scale logic in `ThumbnailService` needs a headless variant: `QImage`-based
(works under `QCoreApplication`) rather than `QPixmap`-based (needs a GUI
platform). This is a small but real piece of work — split the pixel logic out so
both the daemon (QImage→JPEG file) and the GUI (QImage→QPixmap for display) share
it.

Privacy: thumbnails of received private snaps written to a cache dir are readable
by anything that can read your home dir. Gate behind §6.

---

## 5. Build order (stages — each independently shippable/testable)

**Stage 0 — extract headless pixel logic.** Split `ThumbnailService`'s
crop/scale into a `QImage`-based core usable under `QCoreApplication`. No daemon
yet; GUI keeps working. Testable in isolation. LOW risk.

**Stage 1 — daemon skeleton, GUI unchanged.** New `snapclone-daemon` target
(`QCoreApplication`) that constructs `AppController` headless. Quarantine the
`AppContext` `QGuiApplication` state read behind a "has-gui" flag (daemon passes
false → power logic uses a sane default). Daemon runs, connects to the network,
maintains presence — but exposes nothing yet. GUI still runs standalone (still
owns its own core). VERIFY: daemon stays resident, presence updates in its logs.
MED risk (lifecycle, key loading §2).

**Stage 2 — DBus service on the daemon.** Implement `org.snapclone.Dashboard`
(§3) on the daemon: `Contacts()`, `PresenceChanged`, `UnreadTotal`. No thumbnails
yet. VERIFY with `busctl`/`qdbus` from a terminal. MED risk.

**Stage 3 — single-owner cutover.** The GUI stops owning a core and becomes a
DBus *client* of the daemon. This is the highest-risk stage: every GUI call into
`AppController` must be re-routed to DBus or to a thin local client that proxies.
Needs the daemon to be reliably activatable before the GUI. Gate behind a feature
flag so you can fall back to the monolithic app. HIGH risk — most of the work.

**Stage 4 — thumbnails + unread.** Daemon generates thumbnail JPEGs (Stage 0
logic) to the cache dir, exposes `RecentThumbnails`/`LatestThumbnailFor`/
`ThumbnailReady` + `UnreadChanged`. MED risk.

**Stage 5 — surfaces.** (a) Qt-widget dashboard panel (a `QWidget` client of the
DBus interface — reuses `FriendListWidget` patterns). (b) Plasmoid: QML + a small
DBus data source. Both read the same interface; build the widget panel first (no
KDE packaging) to validate the contract, then the Plasmoid. LOW–MED risk.

You can stop after any stage with a coherent product. Notably Stage 2 alone (with
the GUI still owning its core, daemon read-only-mirroring presence) is *not*
sound — two core owners — so the first genuinely shippable dashboard is after
Stage 3. If that's too much, the fallback is the GUI-app-as-service model (the
app exposes the DBus service only while open) which skips Stages 1 and 3 entirely
but loses "works app-closed." Flag this tradeoff to the user.

---

## 6. Security / privacy model (must-resolve, not optional)

This is an **end-to-end encrypted** app. Moving contact identity + decrypted snap
previews onto the session bus and into a cache dir widens the trust boundary from
"this one app" to "every process running as this user." Decisions:

1. **Session-bus exposure is opt-in.** A setting (default OFF) gates whether the
   daemon registers `org.snapclone.Dashboard` at all. Users who don't want a
   desktop dashboard never widen the boundary.
2. **Thumbnail cache is opt-in + scoped.** Writing previews of received private
   snaps to disk is the most sensitive part. Default OFF; when on, write to a
   mode-0700 dir, prune aggressively, and consider excluding snaps marked
   ephemeral/"view once" from ever being thumbnailed.
3. **No message bodies over DBus.** The interface exposes presence, counts, and
   thumbnail *paths* — never decrypted text/media bytes. The dashboard is a
   notification surface, not a reader.
4. **Daemon key handling** per §2 — decide (a)/(b)/(c).
5. **Peer authenticity unchanged.** The daemon uses the same crypto/identity
   path; the DBus layer never sees plaintext keys.

---

## 7. Open questions for the user
- Key-at-rest model for the unattended daemon: §2 (a), (b), or (c)?
- Are "view once" / ephemeral snaps allowed to be thumbnailed at all? (Recommend
  no.)
- Is the phone (Plasma Mobile) the only target, or also desktop Plasma? Affects
  systemd-user vs DBus-activation packaging and the applet form factor.
- Acceptable to ship the Qt-widget panel first (validates the contract) before
  investing in the Plasmoid packaging?

## 8. Decision (recorded)
Pursue Option B (headless daemon) because the core is already a widget-free
QObject that owns the transport — the expensive part is the single-owner cutover
(Stage 3), not the headless core itself. Thumbnails go path-based via a daemon-
written cache dir (Stage 0 + 4), reusing the BLE small-blob thumbnail framing.
All session-bus exposure and on-disk previews are opt-in and default-off because
this is an E2E-encrypted app and the session bus is a wider trust boundary.
