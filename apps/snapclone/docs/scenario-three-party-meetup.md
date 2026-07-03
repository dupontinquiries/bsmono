# Scenario stress-test: three parties, multiple devices each, coordinating a meetup

The scenario: **three parties of people, multiple devices each, all nearby but
physically separated, communicating amongst themselves AND with the party already at
the meeting point.** Does group conversation cover this well, and what would help?

Short answer: the *messaging substrate* is genuinely strong for this — better than I
expected before reading it. The gaps are around (1) **multi-device-per-person**, (2)
**destination/meeting-point as a first-class object** (today location is peer-centric),
and (3) some **coordination affordances** (who's-where, ETA, "I'm here") that the
primitives support but nothing surfaces yet.

---

## What already covers it well

### 1. Group fan-out + sealed-sender + epochs — solid
`snapclone-group` is not a toy. It has: a per-sender ratchet (forward secrecy within
an epoch), membership epochs that re-key on add/remove (a removed member can't read
new content; a new member can't read old), and per-recipient sealed key distribution
(O(1) content regardless of group size). For a meetup group that gains/loses people,
the re-key-on-membership-change is exactly right.

### 2. Partition tolerance — the DAG is the standout
This is the part of your scenario I was most worried about ("nearby but physically
separated… communicating amongst themselves"), and it's the best-built piece.
`dag.rs` is a content-addressed Merkle DAG: each snap names the parent snap-ids its
author had seen, so the graph structure *is* the causal history. Two replicas that
drifted apart while separated find their difference by comparing tips, and a Lamport
clock + deterministic tiebreak gives **every device the same total order** once they
reconverge. So when the two separated parties merge back, nobody sees a scrambled or
divergent conversation — they converge to an identical ordering with no central clock.

### 3. Catch-up after being offline — the mailbox `gaps()` mechanism
`mailbox.rs` tracks per-sender sequence numbers and `gaps()` computes exactly which
`(sender, seq)` entries a device is missing. A party that was out of range comes back,
sees the gaps, and pulls just the missing snaps — not a full resync. This is the
right primitive for "separated, then back together."

### 4. Location awareness exists — Wayfinder
`WayfinderPage` renders contacts by **bearing + log-distance** around you (a compass/
astrolabe, not a road map): "Priya 1.2km NW," online dots, their latest snap as the
marker. For "where is everyone relative to me," this is already a strong answer.

---

## Where the scenario exposes gaps

### Gap A — Multi-device per person is NOT modeled at the group layer ⚠️ (biggest)
Your scenario says "multiple devices each." But `group::Member` is `{ sc_id,
x25519_pub }` — **one identity, one key, per person.** There is no notion that
Priya's phone and Priya's tablet are the *same person*. Consequences:
- If each device has its own sc_id, "Priya" appears as 2–3 separate members. Messages
  sealed to "Priya-phone" don't reach "Priya-tablet." She has to read on the exact
  device a snap was sealed to.
- The roster shows devices, not people — a 3-person/6-device meetup looks like a
  6-member group, and the Wayfinder shows 6 dots, two of which are the same person in
  the same place.
- Re-key churn: a person putting one device to sleep / waking another looks like
  membership change.

Notably, the *rendezvous* layer already anticipates multi-device — the micro-DHT store
comment says "multiple values under one key (multi-device / re-announce)." So the
lower layer expects it; the **group membership layer doesn't model it.** That's the
seam to close.

**What would help:** a person → devices mapping. Two designs:
- *Lightweight (recommended first):* a "device set" per sc_id — one human identity key
  (the published sc_id / signing key) plus N device sub-keys. Group messages seal to
  the **person**; each of their devices can unwrap (the sealed key is additionally
  wrapped to each device sub-key, or devices share the person's group key via a secure
  local link — BLE per §6, or a one-time QR link). The roster, Wayfinder, and ratchet
  all key on the person; devices are an implementation detail underneath.
- *Heavier:* full per-device ratchets with a device-list CRDT (Signed by the person's
  identity key). More correct for independent device compromise, much more machinery.
  Probably overkill for friend-scale.

### Gap B — The meeting point is not a first-class object ⚠️
The whole scenario orbits a **place** ("the meeting point," with one party already
there). Today location is *peer-centric*: Wayfinder shows people relative to you, and
location snaps share a one-shot point. There's no shared **destination** that the
group agrees on and everyone navigates toward.

**What would help — a "Pin" / shared waypoint as a group object:**
- Any member drops a Pin (a lat/lon + label, e.g. "the fountain by the north gate"),
  sealed to the group like any other group snap — so it rides the *existing* group
  fan-out + DAG + catch-up with zero new transport.
- The party already at the meeting point can drop the Pin **at their current
  location** ("I'm here, come to me") — turning their presence into the destination.
- Wayfinder gains a **destination mode**: instead of (or alongside) peer-relative
  bearing, show bearing + distance to the *Pin*, and optionally each other member's
  bearing/distance to the same Pin ("Priya is 400m SE of the pin, Tom is 1.1km N").
  This is the single highest-value addition for this scenario and reuses Wayfinder's
  existing bearing/log-distance math.
- ETA-ish hint: distance ÷ a walking-speed assumption gives a coarse "≈8 min away."
  No routing engine needed (and the app deliberately has no basemap) — straight-line
  is honest for "are they close yet."

### Gap C — No "sub-party" / who-is-with-whom grouping
The roster is flat. Your scenario has natural clusters: party A together, party B
together, party C at the point. Members co-located could be shown as a cluster ("these
3 are together at 51.50,-0.12") rather than 3 separate dots stacked on the map.
- The proximity crate (BLE RSSI) + shared location already provide the raw signal for
  "these members are near each other."
- **What would help:** cluster co-located members in the Wayfinder, and optionally a
  lightweight "we're together" affordance (one tap: "our party is the three of us").
  Low priority vs A and B, but it directly declutters this exact scenario.

### Gap D — Delivery confidence across a partition (UX, not protocol)
The protocol re-delivers via DAG/gaps when parties reconverge (good). But during the
separation, a sender doesn't clearly know *who* has and hasn't received a snap yet —
e.g. "did the party at the meeting point get my 'running late' snap?" The ACK +
outbound-delivered machinery exists per-recipient; in a group it needs **per-member
delivery state surfaced**: a small "2 of 3 parties have seen this" indicator. The data
is there (per-sender seqs, ACKs); it's a UI synthesis.

---

## Concrete recommendations, ranked for this scenario

1. **Shared Pin / destination object + Wayfinder destination mode** (Gap B).
   Highest value, lowest risk: it's a new *group snap type* (lat/lon + label) that
   reuses all the existing group plumbing, plus a Wayfinder rendering mode using math
   it already has. This is what turns "a group chat" into "a meetup tool."

2. **Multi-device-per-person (lightweight device-set)** (Gap A). The correctness gap.
   Without it, "multiple devices each" means duplicate members and messages that land
   on the wrong device. Start with the person-key + device-sub-keys model; link
   devices over BLE/QR (ties into §6's BLE control plane). 

3. **Per-member delivery indicator in groups** (Gap D). Pure UI synthesis over data
   that already exists; big confidence win when parties are separated ("the meeting-
   point party has seen my update").

4. **Co-location clustering in Wayfinder** (Gap C). Nice-to-have declutter; uses
   proximity + location already present.

## Why the foundation makes all four cheap
Every one of these rides existing infrastructure: the DAG handles ordering/partition,
the mailbox handles catch-up, the group ratchet handles sealing, the Wayfinder has the
geometry, the proximity crate has co-location, and BLE (§6) can link devices and carry
the small Pin/location blobs even when IP is partitioned. None require a new transport
or a re-architecture — they're a new *group object type* (Pin), an *identity refinement*
(device sets), and two *UI syntheses* (delivery state, clustering). The scenario is
well-supported at the substrate level; what's missing is the meetup-specific surface on
top of it.

## Honest caveats
- The DAG/group ordering is verified by 73 passing tests, but the *multi-device*
  behavior is untested because the model doesn't exist yet — adding it needs its own
  test suite (device-set seal/unwrap, roster collapse, re-key-on-device-change-suppression).
- "All three nearby but separated" assumes BLE range (~10–30m) for the BLE-assisted
  parts; "separated" across a venue may exceed BLE range, in which case co-location
  clustering falls back to shared-location proximity, not BLE RSSI. Worth deciding
  which signal drives clustering.
- Straight-line distance/ETA is honest but can mislead around real obstacles (a river,
  a building). The app's no-basemap stance means we should *label* it as straight-line,
  not imply routing.
