# SnapClone — Vision

> Calm, private, peer-to-peer ephemeral messaging.

## What this is

SnapClone is a **distributed peer-to-peer snap messenger**. No central server.
No accounts. No cloud. Two devices that can reach each other on the network
can exchange photos and videos that disappear after viewing, without any
intermediary observing the content or the metadata of who-talks-to-whom.

This is intentionally not Snapchat-the-product. It borrows the surface
*idea* — ephemeral images with a view timer — and rebuilds the substrate
in a way that respects the user.

## What we believe

### 1. Messaging should not require a company

A friendship and the messages within it are a personal arrangement
between two people. A messaging app that *requires* a corporate
relationship in the middle has the wrong shape. SnapClone runs entirely
on the user's own devices and network connections; the developers
cannot see who messages whom or what is sent, even if compelled.

### 2. Ephemeral means ephemeral

When a snap expires, it is gone — not "moved to a 'recently viewed'
cache for 30 days for our records." Encrypted blobs on disk are removed
when their view timer ends. No telemetry, no analytics, no "engagement
metrics."

### 3. Save is a feature, not a betrayal

Snapchat famously notifies the sender when a recipient screenshots a
message. This treats the recipient as a suspect. SnapClone takes the
opposite stance: **recipients can save anything they choose to.** Once
you've sent something to a person, you've made a choice to trust them.
If you don't trust them, don't send it. The app will not enforce
behaviour between two adults.

### 4. The UI is for the user, not for the metric

No streaks. No "your friend hasn't snapped you in 3 days!" pop-ups.
No engagement-loop animations designed to compel return visits.
SnapClone is muted, intentional, and calm. The home page does not
deliberately reward visiting the home page.

### 5. Local-first, network-second

The app must remain useful with no internet. LAN discovery via UDP
gossip works on planes, in basements, between hotel rooms. Internet
DHT and relays are optional augmentations, never preconditions.

### 6. Honest engineering

The code is built to be understood. Function names describe what they
do. Comments explain *why*. Architectural decisions are recorded in
[ARCHITECTURE.md](../ARCHITECTURE.md). When we change something
non-obvious, we leave a note for the next reader (often ourselves).

## What this is not

- **Not a Snapchat clone.** No streaks, no Stories competing for
  attention, no Discover feed, no influencer model.
- **Not a "fully decentralised everything" maximalist project.**
  We use OpenDHT for internet discovery — that's a network of nodes,
  not a corporate server, but it's also not pure local mesh. Pragmatism
  over purity.
- **Not a fork of Signal/Wire/Matrix.** Those are excellent for
  durable text/voice. SnapClone is *ephemeral image-first*, which is
  a different design space.
- **Not an attempt to be the world's most secure thing.** We use
  X25519 + AES-256-GCM, we don't have a central key escrow, and snaps
  are encrypted in transit and at rest. We're not, for instance, doing
  formal verification of the protocol. We're trying to ship a good
  messenger.

## Who is it for

Two friends or family members who:
- Want to share moments of daily life with each other privately.
- Don't want those moments archived, mined, or monetised.
- Have either a LAN connection between them OR internet access on
  both ends.
- Don't mind that the app is missing some corporate-app polish in
  exchange for owning the substrate themselves.

## Long-term direction

- **Community-led mesh networking** (USB-C LoRa, BLE rendezvous,
  WiFi-Direct islands). Already scaffolded via the
  `TransportCapabilities` interface so a future `LoRaTransport` can
  drop in alongside `WebSocketTransport`.
- **Mobile parity.** Currently runs on postmarketOS phones; iOS and
  Android ports possible but not on the near-term path.
- **Voice & video calls** (Opus + SVT-AV1 pipelines are already in
  place for the snap path).
- **Group conversations** (current model is 1:1; group support needs
  a key-distribution scheme).

## What "done" looks like

Two people can install SnapClone on their phones, add each other once
via a QR code, and from then on exchange snaps reliably whether
they're on the same WiFi, on the same cell network, or across the
public internet — without ever talking to a third party's server.

The app stays out of their way. It doesn't ask for attention. It
just works.
