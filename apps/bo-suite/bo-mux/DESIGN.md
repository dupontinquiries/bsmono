# Distributed multi-speaker channel mux & coordinator

A protocol + module for turning a roomful of heterogeneous devices
(Mozart B&O speakers, pre-Mozart/ASE B&O speakers, a laptop, a phone — any
running this app) into a coordinated multi-channel playback system.

This document is honest about what's achievable. Read the "Sync reality"
section before getting excited about tight movie surround.

---

## The model

Every device running the app joins a **mesh** over the LAN via mDNS. Each
advertises:
  - a stable node id (uuid persisted per install)
  - its display name + platform (mozart / ase / linux / phone)
  - the **speakers it can drive**: itself (local PipeWire output) and/or
    any B&O REST endpoints it can reach
  - its current role (idle / coordinator / member)

When the user starts multi-speaker playback on one device, that device
**announces** a session. The mesh runs a tiny election so exactly one
device is the **coordinator** at a time:

### Announcer / coordinator election ("latest announcer takes over")

A session is identified by `(term, coordinator_id)`. `term` is a
monotonically increasing integer.

- To start (or take over) a session, a device picks
  `term = max(seen_terms) + 1`, sets itself coordinator, and broadcasts
  `Announce { term, coordinator_id, session }`.
- Any device receiving an `Announce` with a **higher term** than it has
  seen accepts the new coordinator and abandons any prior session. This is
  the "latest announcer takes over" policy you described — it's a
  simplified Raft-style term mechanism without the log/voting, which is
  fine because we don't need consensus durability, just a clear current
  owner.
- Ties (same term from two devices) are broken by lower `coordinator_id`.
- The coordinator re-broadcasts a `Heartbeat { term }` every 1s. If
  members miss ~3 heartbeats, the session is considered dead and any
  device may announce a new term to take over.

This gives: one clear owner, instant takeover when someone new hits play,
graceful recovery if the coordinator drops off.

### Channel claiming

The coordinator broadcasts the **channel layout** it wants for the current
content, e.g. for 5.1: `[FL, FR, C, LFE, SL, SR]`; for your ambient mix:
`[water, birds, pad]` (arbitrary named stems).

Members respond with **claims**: "node X will drive speaker S for channel
C." The coordinator resolves conflicts (two devices claiming the same
channel → it picks one, by link quality / user pinning / first-come) and
publishes the final **assignment map**. Each member then connects ONLY to
the speakers it's responsible for. Unclaimed channels can be:
  - folded into the nearest claimed channel (downmix), or
  - dropped (e.g. skip LFE if no sub), or
  - assigned to the coordinator's own output as a fallback.

A device with bookshelf speakers + a soundbar might claim FL+FR; the phone
claims SL; the laptop claims SR — and the movie's rears play on the phone
and laptop "transparently," as you put it.

---

## The mux: where the source splits

**Coordinator-holds-and-serves** (the v1 choice):

The coordinator owns the source (a local multichannel file, or named
ambient stems). It runs a **channel splitter** that exposes one HTTP
endpoint per channel:

    http://<coordinator>:<port>/ch/FL
    http://<coordinator>:<port>/ch/SL
    http://<coordinator>:<port>/ch/water
    ...

Each endpoint streams a mono (or stereo-pair) PCM/encoded substream of just
that channel. Then:
  - **B&O speakers** are pointed at their channel URL via the same REST
    PlayQueue/stream calls the app already makes (Mozart `PlayQueue`, ASE
    `setData`/stream). The speaker pulls its channel like any network
    source — no B&O multiroom protocol needed, works cross-generation.
  - **Local devices** (laptop/phone running the app) pull their channel URL
    and play it through PipeWire on that device.

Why coordinator-serves rather than each-device-pulls-the-whole-file:
  - the source only needs to exist on one device,
  - the coordinator controls exactly what each channel gets (enables
    upmix, per-channel delay for sync trim, level/EQ),
  - both B&O stacks already know how to pull an HTTP URL — no new speaker
    capability required.

The cost: the coordinator's uplink carries N channel streams. On a LAN
that's fine (even 6×1.5Mbps lossy = trivial; 6×PCM is ~4.6Mbps each, still
fine on wifi for a handful of channels).

---

## Sync reality (read this)

This is the part that separates "works" from "marketing."

**The problem:** each endpoint buffers independently. A Mozart speaker, an
ASE speaker, and PipeWire on a laptop each have their own clock and their
own (drifting, unknown) playout latency — typically 0.3–2s for networked
B&O, ~tens of ms for local PipeWire. Nothing here shares a hardware word
clock.

**What we can do:**
1. **Offset estimation.** Each member runs an NTP-like exchange with the
   coordinator to estimate clock offset (round-trip halving). Gets us to
   roughly ±5–20ms of *clock* agreement on a quiet LAN.
2. **Latency calibration.** The hard part is *playout* latency, not clock.
   v1 approach: a one-time **calibration** where the coordinator plays a
   chirp on each endpoint and the user (or a mic) aligns them — or a
   per-device-type latency profile the user nudges with a slider. Store the
   correction per endpoint.
3. **Scheduled start.** The coordinator picks a future start timestamp T
   (e.g. now + max_buffer + margin) and tells every member "begin playout
   of your channel at T (in coordinator time, minus your measured offset
   and latency correction)."
4. **Drift trim.** Long ambient tracks drift; periodic re-sync via tiny
   per-channel resample/skip nudges keeps them from wandering.

**What this gets you, honestly:**
  - **Ambient / multi-stem (your water+birds): excellent.** Non-transient
    layers tolerate tens of ms of misalignment inaudibly. This is the
    flagship use case and it'll feel magical.
  - **Music spread across rooms (multiroom): good.** Like Sonos/Chromecast
    groups — fine for casual listening, occasional perceptible drift.
  - **Tight movie 5.1 phantom imaging: best-effort, not THX.** We can get
    rears playing the right content roughly in time, which is genuinely
    useful, but we cannot promise <10ms front/rear lock through DLNA push
    to speakers whose clocks we don't own. The UI should set this
    expectation, not hide it.

For reference, AirPlay 2 and Chromecast achieve tight sync with custom
protocols and (for AirPlay) bespoke timing silicon. We're doing best-effort
software sync over vendor REST APIs — a different, more constrained game.

---

## Virtual channel expansion (2.0 → surround)

A standalone **local DSP stage** on the coordinator, applied before the
splitter, independent of all the sync/coordination machinery:

  - **Mid/Side decode** → derive a center (mid) and an ambience/surround
    (side) channel from stereo.
  - **Matrix upmix** (Dolby-Pro-Logic-style): C = (L+R)/√2, S = (L−R)/√2,
    rears = decorrelated/delayed S.
  - **Decorrelation + delay** on the synthesized rears (10–25ms Haas delay
    + allpass decorrelation) so 2.0 content gets a convincing surround
    envelope without obvious comb filtering.

This means even a plain stereo track can fill all your claimed channels.

---

## Module layout (this drop)

  - `protocol.rs`  — message types, term/election logic, assignment map
  - `mesh.rs`      — mDNS advertise + browse, peer table, heartbeats
  - `mux.rs`       — channel splitter + per-channel HTTP serving (sketch)
  - `sync.rs`      — clock-offset estimation + scheduled-start math
  - `upmix.rs`     — 2.0 → surround DSP (pure, testable)

These are designed as a **separate `bo-mux` crate/module** you can fold into
the app later. The coordination + protocol layer is the solid, finishable
foundation; the mux/sync are sketches with the real algorithms outlined and
the honest hard parts flagged in comments.

## Where this plugs into the app

  - The existing mDNS discovery (you just added) already finds B&O speakers;
    mesh.rs extends it to also find *peer app instances*.
  - The existing per-device REST calls (PlayQueue / setData stream) become
    the "point speaker S at channel URL" primitive.
  - A new "Spaces" page in the UI: pick a layout, see devices claim
    channels live, hit play.
