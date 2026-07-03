# SnapClone — Roadmap

This file captures the **strategic direction** — the broad bands of
work we're moving through and the order. For tactical bug-tracking
see [TODO.md](TODO.md).

Status legend:
- ✅ Shipped & stable
- 🔄 In progress
- 📋 Designed, not yet started
- 💡 Idea — not committed

---

## Stage 1 — Two devices can talk reliably  🔄

The minimum viable system: install on two devices, add each other,
exchange a snap, and have it actually arrive.

### Done
- ✅ Identity: stable `sc-XXXXXXXX` IDs + X25519 keys
- ✅ LAN discovery via UDP gossip multicast (port 9877)
- ✅ Internet discovery via OpenDHT (1.28.x)
- ✅ STUN reflexive address detection (16 server list with TCP fallback)
- ✅ UPnP + NAT-PMP port mapping (best-effort)
- ✅ WebSocket transport for snaps (Qt6 native)
- ✅ End-to-end encryption: X25519 ECDH → AES-256-GCM (Rust crate)
- ✅ Storage: SQLite with 11 schema migrations
- ✅ Snap retry & re-sync (SYNC reconciliation when a peer rejoins)
- ✅ Same-LAN-first address ordering (don't waste 10s on hairpin)
- ✅ Loopback / self-id refuse-to-punch guards (anti-feedback-loop)
- ✅ WebSocket keepalive ping every 30s with 3-miss force-disconnect

### In flight
- 🔄 Wire-format videoDurationMs (v4 extends v3 — receivers read it
  only when `version >= 4`)
- 🔄 Mobile video transcode reliability (postmarketOS / SVT-AV1)
- 🔄 Mobile crash investigation (deferred-emit fix for contact picker)

### Open
- 📋 Relay fallback when both peers are behind symmetric NAT (CGNAT)
- 📋 Reliable mailbox-delivery for offline peers (DHT mailbox exists
  but TTL is only 10 min — needs longer-lived store-and-forward)

---

## Stage 2 — The app is pleasant to use day-to-day  🔄

### Done
- ✅ SwipeNavigator: horizontal page flow (Wayfinder ←→ Friends ←→
  Camera ←→ Inbox ←→ Settings)
- ✅ MobileDialog: bottom-sheet on phone, regular dialog on desktop;
  scrim auto-cleanup with 15s safety timer
- ✅ Wayfinder: geographic friend view with per-contact stats,
  online-presence dots, fresh-snap pulse animations
- ✅ ContactsPage with beacon button for nearby-peer discovery
- ✅ Theme palettes (warm muted defaults)
- ✅ View-once timer with save-during-view affordance
- ✅ Recordings → CacheLocation (XDG-compliant; backup tools skip them)

### In flight
- 🔄 Encoding progress meter that doesn't stick at 0% on mobile
- 🔄 NV12 explicit YUV→RGB path (Qt's `toImage()` produces tiled
  garbage on Asahi/aarch64)

### Open
- 📋 Outbox UI surfacing (data exists, no visible mount point)
- 📋 Link composer URL preview (Rust `linkpreview::fetch` exists,
  no Qt UI hookup)
- 📋 Group conversations (key distribution design needed)
- 📋 Voice messages (Opus pipeline exists for video; isolate to
  audio-only)

---

## Stage 3 — Honest performance on real hardware  🔄

### Done
- ✅ AV1 encoding via svtav1enc/vp9enc/vp8enc fallback chain
- ✅ Audio probe (gst-discoverer) so video-only inputs don't hang
  PREROLL on a missing audio pad
- ✅ V4L2 device release polling (fixed 1500ms wait after Qt
  backend full-teardown so USB bandwidth release completes)
- ✅ Stride mismatch + YUV-skew detection on viewfinder frames

### In flight
- 🔄 Mobile transcode timing: SVT-AV1 preset=10 is faster but
  still ~30-60s on FP5 for a 10-second clip
- 🔄 v4l2 buffer pool failure root cause (USB bandwidth contention,
  not memory)

### Open
- 📋 Hardware-accelerated decode/encode where available
  (V4L2 M2M on Fairphone's V4L2-mem2mem driver)
- 📋 Adaptive bitrate based on observed throughput to peer
- 📋 Power management (don't run gossip multicast on battery <10%)

---

## Stage 4 — Network situational adaptiveness  🔄

The app should *notice* its environment and adapt — not have one
hardcoded behaviour and hope.

### Done
- ✅ Reachability detection via QNetworkInformation + 10s SSID
  polling
- ✅ Address classification helper: RFC1918, CGNAT, link-local,
  ULA, loopback (`include/network/AddressClassification.hpp`)
- ✅ CGNAT range detection: log a hint that hairpin will likely fail
- ✅ Same-/24 detection for "same-WiFi" connectivity (skip public
  IP entirely)
- ✅ Exponential backoff on relookup (2s→4s→8s→…→60s)
- ✅ DHT mailbox for offline peer connect requests

### Open
- 📋 Per-peer connectivity profile cache: remember that "Alice always
  needs relay" vs "Bob always works on LAN" so we don't re-discover
  on every startup
- 📋 IPv6 path support (only IPv4 wired today)
- 📋 Multi-path: try LAN AND relay in parallel; first one to
  establish wins, other gets aborted
- 📋 ICE-style candidate gathering with priority-ordered races

---

## Stage 5 — Community-led mesh  📋

Long-term: SnapClone should work off-grid. The transport layer is
designed to be pluggable.

### Designed (interface-only)
- 📋 `TransportCapabilities` struct (bandwidth, latency, max payload,
  store-and-forward semantics) — `include/network/NetworkManager.hpp`
- 📋 `ITransport` virtual interface for future backends

### Open
- 💡 USB-C LoRa add-on transport (very low bandwidth, store-and-fwd)
- 💡 BLE rendezvous mode (range ~30m, useful for "find people
  nearby" at events)
- 💡 WiFi Direct island mode (full-bandwidth on the same physical
  spot without infrastructure)
- 💡 Routing layer that picks among available transports based on
  the snap size / urgency / battery state

---

## Stage 6 — Cross-platform  💡

Currently runs on:
- ✅ Linux desktop (Fedora, Ubuntu)
- ✅ postmarketOS phones (tested on Fairphone 5)

Not on the immediate path:
- 💡 macOS desktop (likely straightforward — Qt6 supports it)
- 💡 Windows desktop (Qt6 supports it, but the camera/v4l2 paths
  need a Windows-Media-Foundation backend)
- 💡 Android (Qt6-on-Android works; would need touch/permission
  rework and likely a separate gradle build)
- 💡 iOS (Qt6 supports it but Apple's restrictions on
  background networking/UDP may force a redesign)

---

## What we are deliberately NOT doing

- ❌ Web client (browsers can't do P2P UDP; would force a server
  back into the architecture)
- ❌ Snapchat-style "stories" feed (deliberately not engagement-y)
- ❌ Public discovery / "find new friends" feature (peer-to-peer
  means you already know who you want to talk to)
- ❌ Cryptocurrency, NFTs, AI-generated filters, or other things
  that scratch nobody's actual itch
