# Memory Leak Analysis — V32.34+ Pass

Audit scope: long-lived collections in core, network, ui, crypto layers.
Method: grep for QHash/QSet/QList members, trace insert + remove paths,
identify maps where insert count grows over time without bounded eviction.

## Fixed in this pass

### `Impl::peerPunchAddr` — was unbounded
**Was:** plain `QHash<QString, PeerAddress>` with only full-clear cleanup.
Grew one entry per peer ever STUN-punched. Each entry ~40-60 bytes.

**Fix:** wrapped in `PunchAddrEntry { addr, insertedAtMs }` struct stored
in `peerPunchAddrV2`. Two defenses combined:
- `setPeerPunchAddr` hard-cap at 1024 entries, batch-evict oldest 25%
- `sweepPunchAddrs` periodic TTL purge (5-min timer, drop entries older
  than 30 min — NAT mappings expire faster than that anyway)

Compatibility shim `PeerPunchAddrView` preserves the `peerPunchAddr.contains`
and `peerPunchAddr.value` read API so call sites don't need rewriting.
Write call sites updated to `setPeerPunchAddr()` so the timestamp is
recorded on insert.

### Per-peer state at disconnect — was retained forever
**Was:** `mailboxPublished`, `relookupCount`, `pendingDhtLookups`,
`activeLookupHandles`, `lookupGeneration`, `pendingPqEphemerals` — all
inserted on lookup/PQ-handshake initiation but never cleared on
disconnect. Slow growth per ever-contacted peer.

**Fix:** `peerDisconnected` lambda now removes these keys. Some are
intentionally NOT cleared:
- `lastAttemptedAddr` — reconnect logic reads it to mark previous-attempt
  failed
- `pqSessionKeys` — design intent is "derive once per peer, reuse across
  reconnects" (perf vs forward-secrecy trade-off taken at V32.27 PQ
  rollout). Filed as future bounded-cap work.

### `SnapRouter::m_lastSyncHandledMs` — new in V32.34+
Added the SYNC dedup window. Initial implementation had no cleanup.
**Fix:** `clearPeerState` (called on peer disconnect via existing wiring)
now also removes this entry. Each entry is just 8 bytes (qint64) but
the principle is the same — bounded by current peer count.

### `SwipeNavigator::m_filteredWidgets` — held dangling pointers
**Was:** raw `QWidget *` pointers added on `installFilterRecursive`
but never removed on widget destruction. Lookup-by-pointer was safe
(Qt reuses addresses → wrong widget, but the contains() call doesn't
match the OLD pointer key, so behavior was correct — but the set
bloated).

**Fix:** `connect(w, &QObject::destroyed, this, [this](QObject *dead)
{ m_filteredWidgets.remove(...); })`. The `this` context means if
SwipeNavigator dies first, no spurious slot fires.

## Audited and clean

- **`stabilityTimers`** — disconnect path takes + deletes. 60s timer
  lambda also cleans up. No leak.
- **`transportToRealId`** — erased on peer disconnect. Bounded by
  current connections.
- **`pendingAddresses`** — removed on connect or address-queue exhaustion.
- **`m_lanFailureCount`** (NetworkManager) — erased on successful
  connect. Per-ever-failed-peer accumulation; modest leak filed for
  future (see "Outstanding modest leaks" below).
- **`s_lastDiscoveryMs`** (NetworkManager static) — periodic prune
  on 60s TTL. Capped by definition.
- **`m_avatarCache`** (WayfinderPage) — keyed by contact ID, bounded
  by contact count (~100).
- **`m_thumbCache`** (InboxWidget) — hard cap of 200 with full-clear
  on overflow. Crude but bounded.
- **`m_chainTimers`** (UxGuide) — explicit delete-on-replace and
  delete-on-completion paths.
- **`m_widgetLayer` / `m_dismissOnTap`** (ModalSurface) — destroyed
  signal connected, auto-clean.
- **`m_pendingOffers`** (SnapRouter) — 5-min timer per entry self-drains.
- **`m_v3PendingAck`** (SnapRouter) — inner map drained as ACKs come in;
  outer cleared on disconnect.
- **`m_recentInbound`** (SnapRouter) — hard cap with FIFO eviction.
- **`m_view`** (GossipDiscovery) — hard cap with `kMaxViewSize`.
- **`pendingPqEphemerals`** (was before this pass) — hard cap with
  oldest-eviction.
- **`seenMailboxNonces`** — cap of 500 with batch eviction.
- **All `new QProcess(this)`** — parented, with explicit
  setParent(nullptr) + deleteLater pattern on early termination.
- **All `QTimer::singleShot`** — every site uses the (ms, ctx, lambda)
  form, so context destruction cancels the lambda. No raw-this leaks.

## Outstanding modest leaks (filed, not yet fixed)

These are real but slow growth — each entry small, growth proportional
to "unique peers ever met" which for normal usage is < 100. Filed
because in extreme scenarios (DHT random walks, test harnesses, very
long sessions with high peer churn) they'd matter.

### `RustCryptoService::m_sessionKeys` and `CryptoService::m_sessionKeys`
v1 X25519 session keys. Inserted on handshake, NEVER removed. Same
"reuse across reconnects" trade-off as pqSessionKeys. ~32 bytes/entry
+ Qt overhead.

**Recommended fix:** add a cap (e.g. 256 peers) with LRU eviction.
Requires touching CryptoService/RustCryptoService API (currently
no public removal method). LRU needs a parallel timestamp map or
linked list. ~half-day of work.

### `Impl::pqSessionKeys`
v2 hybrid (X25519 + ML-KEM) session keys. ~32 bytes/entry. Same
design rationale as v1. Same recommended fix shape.

### `NetworkStats::m_byPeer`
Per-peer transport-class profiles. Each `ClassArray` is ~6 enum values
× ~32 bytes per Profile = ~200 bytes. Singleton holding state for
ever-seen peers. ~20KB at 100 peers, ~200KB at 1000.

**Recommended fix:** TTL-based eviction in a periodic sweep. Profile
data older than 7 days isn't useful for ranking anyway.

### `NetworkManager::m_lanFailureCount`
Per-peer LAN-attempt-failure counter. Erased on successful LAN
connect, but never on TTL or disconnect. A peer who's only ever
reached via relay would accumulate an entry forever.

**Recommended fix:** add a periodic prune similar to
`pruneDiscoveryMap()` — drop entries that haven't been touched in
30 minutes.

## Audit method for future passes

To re-run this analysis after future changes:

```bash
# Find all long-lived collections in non-test code
grep -rn "QHash\|QSet\|QList\|QMap" include/ src/ \
  | grep -E "m_|s_|static" \
  | grep -v "test\|Test"

# For each match, identify insert sites:
grep -rn "<MAP_NAME>\.\(insert\|append\|push\)\|<MAP_NAME>\[" src/

# For each match, identify removal sites:
grep -rn "<MAP_NAME>\.\(remove\|erase\|clear\|take\)" src/

# Compare: do insert sites have corresponding remove sites?
# If a map only has clear() (whole-map cleanup) and no per-key
# remove, it's a probable leak.
```

Heuristics that flag a likely leak:
- Map keyed by `peerId.value` (QString) with no remove on disconnect
- Map keyed by `contentHash` or `snapId` with no remove on
  delivery/expiry
- Cache without a size cap or TTL sweep
- Static collections in singletons with no cleanup
- Lambda-captured `QHash *` or `QSet *` that adds but never removes
