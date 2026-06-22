# Design decisions

Decisions we've made and the rationale behind them. The point of
this file is to **prevent re-litigating** decisions — if a question
comes up that's already been answered, it should be findable here.

Each decision is a short rationale plus the alternatives we
considered and rejected.

## ADR-001: Local-only architecture

**Decision:** the `ambient-learner` process never opens TCP sockets,
never makes HTTP requests, never does DNS lookups.

**Why:** trust boundary minimization. We have access to thermal
zone sensors that include hardware identifiers; combining that with
location/internet access creates a fingerprinting surface we don't
want responsibility for. The "network-aware sibling" pattern lets
the user pick which network app to trust, and we get the benefit of
their work via local IPC without inheriting their risks.

**Rejected alternative:** a small built-in HTTP client for weather
data. Would have been ~50 lines and easy. We didn't want it to
become 500 lines and a credentials store.

## ADR-002: Three UI backends, runtime-selected

**Decision:** `--ui gtk|iced|qt` picks the UI; one binary contains
all three (feature-flagged).

**Why:** GTK for desktop maturity, Qt for mobile-Linux phones,
Iced as a stretch goal for Wayland-native. Most distros' default
package can ship GTK; mobile distros can use the Qt build.

**Rejected alternative:** separate binaries per UI. Would have been
cleaner but inflated the build matrix.

## ADR-003: WorldState via Arc<RwLock<...>> with poisoned-lock recovery

**Decision:** shared mutable state lives in `Arc<RwLock<SystemState>>`,
accessed via `world_state::read()` and `world_state::write()` helpers
that automatically recover from `PoisonError`.

**Why:** simple, std-only, works across threads, recovers from a
panic in one worker without bringing down the rest. Read-heavy
workload (UI reads many times per write).

**Rejected alternatives:**
- `parking_lot::RwLock` — drops the poisoned-lock semantics; we
  decided we wanted to keep at least one safety net.
- Actor model with channels — overkill for the structure of this
  data (small, frequently read, mostly written by one worker).
- `Arc<Mutex<...>>` — write-only contention would dominate on UI
  ticks.

## ADR-004: SQLite via `db_writer::DbWriter` with batching

**Decision:** writes go through a dedicated writer thread that
batches into `BEGIN IMMEDIATE / INSERT × N / COMMIT` transactions
with savepoint-per-row.

**Why:** 100ms tick × 4-53 sensors = 240-1320 inserts/min would
otherwise hammer the disk with `fdatasync()` calls. Batched WAL
mode is ~50× faster.

**Rejected alternatives:**
- Sync writes on the sensor thread — easiest but UI-blocking.
- Async with `tokio` — too much new infrastructure; mpsc channel
  + one thread is simpler.
- Discard write rate by sampling — loses data we want for retraining.

## ADR-005: D-Bus on session bus, never system bus

**Decision:** we publish `org.ambientlearner.Service1` on the
**session bus** only.

**Why:** session bus is per-user-login, reachable without polkit,
no root needed. System bus would require a polkit rule and a
service file, and would also let any process on the system see our
data even when our user isn't logged in. Wrong default.

**Rejected alternative:** dual-publish on both buses. Adds
complexity without real benefit for our use cases.

## ADR-006: XDG inbox in addition to D-Bus

**Decision:** consumers can drop JSON files into
`$XDG_DATA_HOME/ambient-learner/inbox/pins/` and we'll ingest them
via inotify.

**Why:** D-Bus integration requires a binding library. Many
publishers (shell scripts, cron, photo organizer post-hooks) can't
easily link one. File-drop is the lowest-friction interface for
those cases.

**Rejected alternative:** D-Bus only. Would have excluded a large
class of useful integrations.

## ADR-007: Public Unix socket emits, doesn't accept

**Decision:** `$XDG_RUNTIME_DIR/ambient-learner.sock` is one-way:
we push newline-JSON events out, we don't read from connected
clients.

**Why:** "stream out + accept commands" needs proper protocol
framing and that grows into a half-baked custom RPC. D-Bus already
exists for commands; the socket is for streaming.

**Rejected alternative:** bidirectional protocol. Started writing
it, realized it duplicated D-Bus, removed it.

## ADR-008: Snapclone-bus federation as client, not server

**Decision:** we connect to a separate `snapclone-bus` daemon if
it's present at `$XDG_RUNTIME_DIR/snapclone-bus.sock`. We don't run
the daemon ourselves.

**Why:** the daemon is owned by a different project. Bundling it
or auto-starting it would create a hard dependency we don't want.
Our client connects best-effort; if the daemon isn't there we run
standalone.

**Rejected alternative:** spawn the daemon ourselves. Too much
coupling; what version do we spawn? Who manages updates?

## ADR-009: Screen-temp bitfield in `/run`, not D-Bus

**Decision:** the 2D heat map is published as a binary file in
`$XDG_RUNTIME_DIR/ambient-learner/` at 4 Hz, with metadata
sidecar JSON.

**Why:** mmap-friendly, polling-friendly, language-agnostic. D-Bus
would force every consumer to use a D-Bus binding for a stream of
high-frequency binary data — exactly what D-Bus is bad at.

**Rejected alternatives:**
- A D-Bus property — high-frequency property updates are wasteful.
- A D-Bus signal — consumers would still need a binding, and signals
  buffer/drop differently per implementation.
- A Unix socket — already used for events; mixing a 4Hz binary
  stream into a JSON event channel would complicate parsing.

## ADR-010: Adaptive screen-temp resolution

**Decision:** the publisher picks its grid size each tick based on
`min(2·√N, 2/σ)` cells per dimension, capped at 64×96 and floored
at 4×6. Consumer hint files in the publish directory can request
higher resolution.

**Why:** sensor count and kernel σ both bound spatial information
content. A small-N device has no business publishing 32×48
interpolation artifacts; a 53-sensor mobile *can* benefit from
finer cells if its kernels are narrow enough.

**Rejected alternatives:**
- Fixed 10×16 always. Wastes work on starved devices, under-uses
  rich ones.
- Per-consumer file-based publishing — too much complexity for one
  multiplexed channel.

## ADR-011: No temporal super-resolution

**Decision:** we don't add a Kalman filter or other temporal
super-resolution step. Per-cell EMA smoothing is the maximum
amount of temporal logic in the publisher.

**Why:** the right filter parameters are application-specific
(touch input wants low latency; grip detection wants smoothness).
A single choice would serve some consumers poorly. Consumers
that need it have the raw stream and can run their own filter.

**Rejected alternative:** add a Kalman filter with configurable
parameters. Documentation burden, parameter-tuning rabbit hole,
narrow consumer audience.

## ADR-012: Thermal reading unit auto-detection (with per-sensor cache)

**Decision:** detection happens on the *first* plausible read per
sensor; subsequent reads from the same sensor use the cached
divisor. Detection itself tries three divisors (1, 1000,
1_000_000) and picks the one that lands in the physical silicon
range.

**Why:** mobile vendor kernels are inconsistent about whether
sysfs thermal values are millidegrees or microdegrees. A static
divisor produces 900°C readings on some devices.

**Why per-sensor caching now:** initially we did per-tick
detection because "adds state" felt wrong. In practice the
per-tick approach has a real failure mode: a sensor reading near
a magnitude boundary (e.g. 65 ↔ 65,000 across consecutive polls)
gets reinterpreted at ~1000× different scales. Caching the
first plausible divisor and committing to it fixes this without
adding meaningful complexity.

**What about a sensor that "boots cold" and reads zero
temporarily?** That was the original concern with caching. In
practice: zero in any unit is in-range (it's 0°C, which is fine
for silicon), so it gets cached as the standard millidegrees
divisor. The first nonzero read from that sensor either confirms
the cache (sensor reports in millidegrees, all good) or — in the
pathological case where the sensor *is* actually in
microdegrees and the boot value of 0 misled us — the cached
divisor produces a value that gets clamped to the physical
range. Worse than detection on a hot value, but not catastrophic:
no out-of-range outlier reaches the ML model.

**Known limitation:** a value like `65` is still genuinely
ambiguous (0.065°C if millidegrees, 65°C if raw degrees). The
first read locks in the millidegrees interpretation. If that's
wrong, every subsequent read of that sensor is also wrong by a
factor of 1000. Acceptable: the wrong interpretation still
produces a clamped, in-range value rather than a 900°C outlier,
and a one-time recalibration corrects the model.

**Rejected alternatives:**
- Static `/ 1000`. Worked for desktop, broke mobile.
- Per-sensor cache with timeout/invalidation. Adds complexity
  to handle a failure mode (sensor changing units mid-session)
  that doesn't happen in practice.

## ADR-013: Backup uses SQLite online backup API, not fs::copy

**Decision:** `Database::create_consistent_backup` uses
`rusqlite::backup::Backup::new(...).step(-1)` to snapshot.

**Why:** `fs::copy` of a SQLite file misses the `-wal` and
`-journal` sidecars. A backup taken right after a crash can be
missing recently-committed transactions or partially applied.
The online backup API knows about all three files.

**Rejected alternatives:**
- `fs::copy` plus copying sidecars. Race-prone.
- `VACUUM INTO`. Works but takes the source lock for the duration.

## ADR-014: zbus 4.0 pinning

**Decision:** Cargo.toml pins `zbus = "4.0"`.

**Why:** zbus 5.0 renamed `SignalContext` to `SignalEmitter` and
made other minor API changes. We tested against 4.0; bumping is
straightforward but unjustified mid-stream. Will move to 5.0
during a planned upgrade cycle.

## ADR-015: Three UI backends play to their respective strengths

**Decision:** GTK, Qt, and Iced are each developed to play to
their own design idioms rather than racing toward feature parity.

**Why:** "the same five pages in three frameworks" gives three
mediocre experiences and triples the maintenance burden. Each
framework has things it does well; users picking that framework
should get the version that uses them.

**What each backend leans into:**

  * **GTK** — desktop-native, libadwaita conventions, HeaderBar +
    Stack patterns. Boring, correct, well-integrated on GNOME.
    This is the safe default and the "looks like every other
    GNOME app" choice.

  * **Qt/QML** — rich animations, mood theming, swipe navigation,
    breathing dots, glass effects, Canvas-painted seasonal
    backgrounds. The QML scene graph rewards this; we lean in.
    Primary mobile (Mobian / Phosh) target.

  * **Iced** — Elm-architecture composability, pure-function-of-
    state views, Canvas-as-pure-function. We use it for **dense
    data displays**: multi-series strip charts, sparklines,
    keyboard-first power-user navigation, fast info-density.
    Touch is first-class (big hit targets, bottom tabs) but the
    differentiator from Qt is *what* is on screen, not *how*
    you interact with it.

**Touch is non-negotiable across all backends.** Even where a
backend's idiom is desktop-focused (GTK with HeaderBar), every
interactive element has a tap target ≥48 dp. Keyboard shortcuts
in Iced are *additive* — present on desktop, ignored on mobile
without harm.

**Concrete consequences:**

  * Iced gets a Canvas-based sparkline + multi-line strip chart +
    **calibration scatter plot** on its Chart tab. The scatter
    plot is a debugging viz that's genuinely useful (and
    natural in Iced's pure-function-of-data idiom).
  * Qt instead gets a polished animated history page with
    breathing dots and now a **pressure gauge** with smoothly
    animated needle.
  * Iced gets keyboard shortcuts (`1`-`5` jump tabs, `c`/`l` open
    dialogs, `Esc` closes) **and tap-and-hold on Model rows** to
    expand a per-sensor history sparkline. Qt doesn't bother
    with keyboard shortcuts — its swipe-and-tap nav is already
    first-class.
  * GTK stays the "everything you'd expect from a GNOME app"
    fallback. Not feature-rich; not unfunctional.

**Rejected alternatives:**
  * Removing one or two backends. Loses the abstraction-validation
    benefit of multi-backend.
  * Common abstraction layer. Doubles the work and ends up at
    lowest-common-denominator UX.

## ADR-016: Separable Gaussian for screen-temp kernel

**Decision:** the screen-temp kernel sum uses the separable form
`G(x, y) = G_x(x) · G_y(y)` instead of the joint 2D form.

**Why:** the 2D Gaussian is `exp(-(dx² + dy²) / 2σ²)` which factors
into one 1D Gaussian per axis. Computing the 1D weights once per
sensor and combining with multiplies turns the per-cell cost from
one `exp()` into one multiply. `exp()` is ~30-50× more expensive
than a multiply on most CPUs.

Combined with bounding-box loop bounds (only iterate cells within
3σ of the source) the kernel sum got ~10-30× faster on typical
inputs (32×48 grid, σ≈0.15, 53 sensors).

**Why now:** the screen-temp publisher is the single most CPU-
expensive periodic task in the app. 4Hz publishing × 32×48 cells
× 53 sensors = 326K exp() evaluations/sec was the dominant idle-
state CPU cost on mobile. Reducing this directly improves battery
life.

**Rejected alternatives:**
- SIMD vectorization (auto-vec only). The separable form unlocks
  this naturally; we leave it to LLVM and the target CPU rather
  than writing intrinsics by hand.
- Approximating `exp()` with a polynomial. Faster but tricky to
  bound the error; the separable form already gives enough headroom.
- Caching the kernel weights between frames. Possible if σ is
  static, but consumer hints can change resolution at runtime —
  and the cache invalidation logic isn't worth the complexity for
  a 4Hz task.

**Kept the reference implementation** (`apply_gaussian_kernel_reference`)
as `#[allow(dead_code)]` so a future regression test can verify
the separable form remains numerically equivalent.

## ADR-017: DB writer blocks indefinitely when idle

**Decision:** the `DbWriter` thread uses `recv()` (no timeout) when
its batch is empty, and `recv_timeout()` only when there's pending
work to flush.

**Why:** previously the writer woke every `BATCH_INTERVAL` (250ms)
even when the channel was idle. On a phone where the CPU enters
deep-idle states after ~10ms of inactivity, 4 writer wakeups/sec
prevented the deepest sleep modes. Blocking indefinitely lets the
CPU sleep until a producer actually pushes a row.

**Cost:** none in throughput terms — the moment a row arrives we
start the deadline timer and behavior matches the old logic.

**Rejected alternative:** an `Arc<AtomicBool>` "wakeup" flag the
producer toggles. Worked, but `mpsc::sync_channel::recv()` already
does exactly this internally on a `Condvar`. Reinventing it would
have added complexity for no benefit.

## ADR-018: Hint dir mtime check before rescan

**Decision:** the screen-temp worker stat()s the hint directory
each tick and only re-reads + parses the hint files when the
directory's mtime has changed.

**Why:** the hint files are rescanned once per second by default,
which on a healthy system finds zero hint files (no consumers
have asked for higher resolution). Reading an empty dir is cheap
but not free; on a system with multiple hints it's N file reads
+ N JSON parses per second. The mtime check is one stat() —
microseconds.

**Known limitation:** if a consumer rewrites its hint file
*in place* (without rename), the directory mtime won't change
and we'll miss the update until the next legitimate change. The
SCREEN_TEMPS.md docs recommend the write-temp-then-rename pattern
which does update dir mtime; in-place rewrites are an out-of-spec
publisher behavior. Acceptable risk.
