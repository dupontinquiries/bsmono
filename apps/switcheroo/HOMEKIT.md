# HomeKit in switcheroo

**Status (2026-09-05): pairing is implemented, unverified against hardware.**
`crates/hap-core` is HAP, controller side: TLV8, SRP-6a (verified against
RFC 5054's vectors), pair-setup M1–M6, pair-verify M1–M4, the encrypted
session, HAP-BLE PDUs with fragmentation, all headless and tested against
an in-process accessory (`hap_core::sim`). `switcheroo-egui/src/hap.rs`
drives it over btleplug; the pairing record lives in
`~/.local/share/switcheroo/pairing-<address>.hap` (0600) beside this
install's controller identity (`controller.hap`). Back that directory up:
losing it means resetting the plug.

What a real WSP100 will settle: two SRP conventions named in
`hap_core::srp::Client::process` (`H(g)` unpadded in the client proof,
`K = H(S)` as the HKDF input). An accessory rejecting M3 — an error at
state 4 in the journal — points at those two lines and nothing else.
The BLE fragment size is `SWITCHEROO_HAP_MTU` (default 100) because
btleplug does not expose the negotiated MTU; raise it if pairing is slow.

**Since 2026-09-06: the controller asks what an accessory IS.**
`hap_core::signature` reads `Characteristic-Signature-Read` (§7.3.4.13) —
type, service, properties, format — and `Controller::probe` walks the
characteristics switcheroo cares about (`On`, `Outlet In Use`,
`Brightness`, `Identify`, `Name`). That is the difference between a plug
and a switch: an **outlet** reports whether something is plugged in and
drawing, and `On` alone cannot say a lamp is unplugged. It also gives the
controller **Identify** — write 1 and the accessory blinks, which is HAP's
own answer to "which of these three identical plugs is the lamp" — and the
owner's **Name**.

Properties are load-bearing, not decoration: `Outlet In Use` is read-only
on every accessory that has it, so a UI drawing it as a toggle offers a
control that cannot work. The probe refuses to invent capabilities — a
characteristic the accessory does not expose is skipped, and a probe run
before pair-verify is an ERROR rather than an empty answer, because
"insufficient authentication" from every characteristic would otherwise
read as "this accessory has no controls".

One session answers all of it (`hap::inspect`): each pair-verify is a
Curve25519 exchange plus two signature round trips over BLE, so asking
per question would turn opening a row into four handshakes. The row shows
"outlet · identify · nothing plugged in" and grows a **blink** button only
when the probe said Identify is there and writable.

**Unpairing now tells the accessory.** It used to delete the local record and
stop, which left the plug paired to a controller that no longer exists:
still claimed, still refusing pair-setup, recoverable only by the factory
reset this file warns about — arrived at by pressing a button labelled
"forget this pairing". `hap_core::pairings` implements Remove Pairing and
List Pairings (§5.11), and the order is the decision: the accessory is told
FIRST and the record deleted only on success. The other order cannot be
recovered from — with the keys gone, the controller cannot ask again. A
failure leaves both sides paired, which is a state to retry from, and the
row says so.

Removing your own pairing ends the session in the same breath as the
reply, so the controller treats a session that vanishes after a good M2 as
success. The simulator does the same, and the test proves the round trip:
pair → verify → list (one admin controller) → remove → the stale record no
longer verifies → the accessory accepts a fresh pair-setup.

In the app: a HomeKit row shows **pair…**, which opens a dialog whose first
line says what pairing costs (one pairing per accessory; an Apple Home
loses it; factory reset to get it back), with the setup code and a
checkbox that must both be right before **Pair** is enabled. Paired rows
get the same ON/OFF toggle as a GATT switch and an **unpair**. Schedules,
the command box ("lights off", "turn the lamp on by 9pm", "when I get
home…") and the media strip are described in `switcheroo-core`'s
`schedule`, `intent` and `media` modules.

The rest of this file is the reasoning from before the implementation. It
is kept because the constraints it names — one pairing per accessory, the
key-loss cost, the fabric-oracle design as the eventual shape — are still
the constraints.

---

# Why the Wemo WSP100 couldn't be switched (2026-09-01)

switcheroo *finds* the WSP100 and every other HomeKit accessory, and
identifies it correctly. It cannot turn one on. This file is here so that
gap is a documented decision rather than a mystery.

## The short version

HomeKit-over-BLE is not "a GATT characteristic with a vendor UUID". It is
a full protocol (HAP, the HomeKit Accessory Protocol) with a mandatory
cryptographic handshake. An accessory will not honour a write to *any*
characteristic from an unpaired, unencrypted controller. There is no UUID
to look up that gets around this — the refusal is the design.

## What pairing actually requires

Pair Setup, once per accessory:

1. Read the 8-digit setup code off the plug (printed on the body and on
   the sticker in the box).
2. SRP-6a password-authenticated key agreement (3072-bit group, SHA-512)
   using that code, proving to the accessory that the controller has it
   without sending it.
3. Exchange long-term Ed25519 identity keys over the SRP-derived session,
   encrypted with ChaCha20-Poly1305 and HKDF-SHA-512 derived keys.
4. Persist the accessory's public key and our own long-term key pair.

Pair Verify, on every reconnect:

5. Curve25519 ECDH, mutual Ed25519 signature check against the keys stored
   in step 4, then a fresh ChaCha20-Poly1305 session.

Only then do characteristic reads and writes work, and they are themselves
framed in HAP PDUs inside the encrypted session rather than being raw GATT
values.

## Why this is not a small patch

- It is a real cryptographic implementation. Getting SRP or the nonce
  handling subtly wrong produces an accessory that pairs once and then
  fails to verify, which is miserable to debug against hardware you have
  to physically reset between attempts.
- It needs persistent key storage with the security properties that
  implies — losing the long-term keys means factory-resetting every plug.
- An accessory only accepts **one** pairing at a time unless it is set up
  as a bridge. Pairing a WSP100 with switcheroo means **unpairing it from
  the Home app**, which is a genuinely destructive thing to do to
  someone's house and needs a very loud confirmation flow, not a button.

That last point is the real reason this is not simply queued as the next
task. The useful version of HomeKit support is probably *not* "switcheroo
becomes a HomeKit controller" but one of:

- **Talk to the Home hub instead of the accessory.** If there is an Apple
  TV / HomePod on the network, it already holds the pairing. This avoids
  stealing the pairing entirely.
- **Use Matter where the device supports it.** Newer Wemo hardware speaks
  Matter, which has a saner multi-admin story — a device can be commissioned
  by several controllers at once, which is exactly the property missing above.
- **Ship the honest UI** (this is what exists now): find the plug, name it,
  say precisely why it cannot be switched, and switch the things that can be.

## What does work today

Plain GATT switches exposing the assigned Automation IO service (0x1815)
with a digital-output characteristic (0x2A57): connect, write one byte, done.
No pairing, no crypto. Most cheap non-HomeKit BLE relays and plugs are this.
See `crates/switcheroo-egui/src/ble.rs`.

## Status of the radio code

Written against btleplug's documented API. **Not yet run against physical
hardware by its author.** The discovery path is conventional and likely
correct; the 0x2A57 write path is the part most likely to need adjusting
once a real plug is in front of it, since a given vendor may use a
proprietary characteristic instead of the assigned one. When that happens
the write fails cleanly with "no Automation IO digital-output
characteristic on this device" rather than silently doing nothing.


## Field observations (2026-09-01, from the reporter)

Two things came back from a real WSP100 that change the picture above.

**Connecting alone turns the plug off.** A bare, unauthenticated BLE
connect — no pairing, no command written — powers the load down. If that
reproduces reliably then "off" is already reachable without HAP, and a
deliberate connect-then-disconnect would implement one half of the switch.
It is NOT wired up here, because shipping it as if it were a real off
command would be lying about what the app is doing, and because half a
switch presented as a whole one is worse than an honest refusal. But it is
worth confirming: a reliable half is more useful than nothing, clearly
labelled.

Worth noting plainly that this is a security flaw in the accessory: anyone
in radio range can power-cycle the load with no credential at all.

**The reporter uses no Apple devices.** That removes the objection this
document leans hardest on. If nothing else holds the pairing, pairing with
switcheroo steals nothing, and implementing HAP becomes an ordinary
(large) protocol task rather than a destructive one.

Their suggested design is better than every option listed above and should
be the plan of record: let **one known node on the bo-suite fabric hold the
pairing and act as an oracle** for everyone else. That is structurally what
an Apple TV or HomePod does — it sidesteps the one-pairing-at-a-time limit
for every client except the hub, keeps the long-term keys in exactly one
place with one backup story, and means a phone joining the fabric gets
control without its own pairing ceremony. The `Backend` seam in
switcheroo-core is already the right shape for this: a fabric-oracle
backend submits the same `Command`s and returns the same `Event`s as the
local radio, and the UI does not learn the difference.

# Fairphone hardware: what it can and cannot do (checked 2026-09-05)

The operator asked whether the Fairphone has the hardware. Checked against
vendor and spec sources rather than assumed, because the two radios involved
have opposite answers.

| | Fairphone 5 | Fairphone 6 (Gen. 6) |
|---|---|---|
| SoC | Qualcomm QCM6490 | Snapdragon 7s Gen 3 (SM7635) |
| Bluetooth | **5.2 + LE** | **5.4 + LE** |
| Thread / 802.15.4 | **no** | **no** — Phone Scoop's spec sheet lists `Thread: –` explicitly |
| NFC | yes | yes |

Fairphone's own Gen 6 spec page lists "Bluetooth 5.4 LE" and no 802.15.4 of
any kind; the FP5 support pages and manual list "Bluetooth 5.2 + LE".

## What that means for this app

**BLE control works, and that is the path.** The WSP100 is a Thread
accessory that speaks HAP over BLE when no Thread network is present —
Belkin's own line is "Bluetooth compatible (used if Thread isn't
available)". Pair-setup, pair-verify and writing the On characteristic are
all HAP-over-BLE, and BLE 5.2/5.4 LE is more than enough. Nothing about the
phone blocks it.

**Thread provisioning does not.** No phone has an 802.15.4 radio — not the
Fairphones, and not iPhones either; on Apple's side it is the HomePod and
Apple TV that carry Thread, which is exactly why HomeKit asks for a "home
hub". Provisioning the plug onto a Thread network means a border router:
an 802.15.4 dongle (nRF52840 and similar) running OpenThread `otbr`, on the
phone over USB-OTG or on any other machine on the LAN. The provisioning
itself still goes over BLE — a write to the Thread Control Point
characteristic `0x0704` — so switcheroo could drive it once a dataset
exists, but it cannot manufacture the network.

So: full function over BLE from the phone alone; Thread is an optional
upgrade that needs one piece of hardware the phone does not have.

## The one-radio consequence, now handled

A phone has ONE Bluetooth radio, and switcheroo runs two BLE stacks:
`btleplug` for the device scanner and `bluest` (under `hap-controller`) for
HomeKit. The scanner is a continuous duty cycle by design, and a discovery
session held while a multi-round pairing runs is how a pairing times out on
a plug sitting a metre away.

`switcheroo_core::radio` is the arbiter: a non-blocking lease with a
priority. A pairing PREEMPTS the scanner and the grant reports what it
interrupted, so the caller restores it afterwards rather than guessing; a
scan is never allowed to preempt a pairing, because a dropped scan costs a
stale list for a few seconds and a dropped pairing usually means putting the
plug back into pairing mode by hand. Advisory rather than a mutex so a
thirty-second pairing never blocks the UI thread. 6 tests.

## Commands worth running on the device

If you want to confirm the radio end-to-end on the phone rather than trust a
spec sheet:

    bluetoothctl show                 # adapter present, powered, and its BT version
    btmon &                           # live HCI trace while pairing
    bluetoothctl scan on              # does the WSP100 advertise at all
    hcitool dev                       # adapter listed at the HCI layer

The plug must be blinking white/orange (pairing mode) to advertise its HAP
service. The earlier field report showed it at **-78 dBm and "out of
range"** — that is a real signal problem before any protocol question, so
worth getting within a metre or two first.


## The advertisement (2026-09-07)

switcheroo learned a plug's state only by connecting: pair-verify, then a read. So a plug
switched at the wall, by the Home app or by a schedule showed the old state until someone
pressed ↻. `classify` looked at service UUIDs and nothing looked at the rest of the advert.

But a HAP accessory broadcasts a **Global State Number** and bumps it whenever a
broadcast-enabled characteristic changes — the accessory telling everyone in radio range
"something about me is different", passively, with no pairing, no connection and no key.
`hap_core::advert` parses the regulatory advert (spec §7.4.2.2: device id, category, GSN,
config number, compatible version, and the not-paired flag) and `GsnTracker` turns a stream
of them into First / Same / StateChanged / Reconfigured.

Three decisions worth keeping:

* **First sighting is not a change.** Treating it as one refreshes every accessory on every
  scan, which is the polling this exists to avoid.
* **Keyed by the advert's HAP device id, not the BLE address.** An accessory that randomises
  its address is still the same accessory; keying on the address would read as a new one on
  every rotation, i.e. `First` forever and never a change.
* **An encrypted notification (type 0x11) is reported, not skipped.** It means an event IS
  being broadcast and we lack the Broadcast Encryption Key to read it — different
  information from "no event", and conflating them is how a controller quietly stops
  noticing changes. Treated as "something changed", which is all the GSN would have said.

In the app: the scanner feeds every manufacturer-data blob through the parser, a change
emits `Event::AccessoryChanged { id, reconfigured }`, and the hub marks the row's power
STALE rather than clearing it — "was on, may have changed" is more use than "unknown", and a
row that blanks on every advert flickers. The row says "changed at the plug — refresh to
read it" and the next successful read clears it. A `reconfigured` advert (config number
moved: firmware update or factory reset) additionally drops everything the capability probe
learned, because it describes a layout that may no longer exist and a reset takes the
pairing with it.

Not done: reading the encrypted notification itself, which needs the Broadcast Encryption
Key from the Pairings characteristic and its refresh protocol; and acting on a change
automatically. Right now a stale row waits for the operator — auto-refreshing on every
advert would reconnect to every plug in the house whenever any of them moved, which is worse
than the polling it replaces.


## The pairing dialog stops guessing

The dialog showed one paragraph: *a HomeKit accessory holds ONE pairing… pairing it here
removes it from that Home… getting it back means a factory reset*. True of HomeKit, wrong
about most individual plugs — one out of its box is claimed by nobody and nothing is lost —
and a warning that is wrong half the time is one people learn to click past. This one guards
the destructive path.

The advertisement answers it. The not-paired flag is the accessory's own statement about
whether it is claimed, so `Outlook::of(unpaired, we_hold_pairing)` gives four cases and each
says something different:

* **Ready** — unclaimed. Nothing is lost.
* **ReplacesOurs** — we hold it. Re-pairing replaces our own record; recoverable, and worth
  saying, since the old paragraph implied otherwise.
* **ClaimedByAnother** — the one case the scary paragraph is actually about. Pair-setup will
  be REFUSED until the plug is factory reset.
* **Unknown** — nothing heard. Say so rather than guess; both guesses are wrong somewhere,
  and this is where the general warning belongs.

The confirmation tick now appears only for the destructive cases. Asking someone to confirm
"I understand this replaces any pairing the plug already has" about a plug that has none
teaches them the tick means nothing — which is what it would mean by the time it mattered.

`Event::Advertised` carries the flag and the category on every advert. It is deliberately
NOT journalled: it arrives several times a second and reports no change, so it would drown
everything else. `AccessoryChanged` is the line worth keeping.


## Acting on a change, without saturating the radio

Last step left a stale row waiting for the operator, on purpose: reading a HomeKit accessory
means taking the radio from the scanner, a Curve25519 pair-verify and a characteristic read —
about a second of exclusive radio. Doing that on every advert means a house with eight plugs
reconnects to all eight whenever any one of them moves, and the scanner never runs. That is
worse than the polling it replaced.

`switcheroo_core::refresh` decides instead. Six rules, each with a reason:

* **One at a time** — the radio is exclusive (`radio::Radio`), so the scheduler picks one and
  the rest wait.
* **Never over a session** — a dropped scan costs a stale list; a dropped pairing costs the
  attempt and usually a trip to the plug to put it back in pairing mode.
* **The visible row first** — its staleness is the staleness the operator can see. Among the
  rest, the one stale longest, so no row is starved by newer changes elsewhere.
* **Settle first** — a wall switch or a dimmer emits several adverts in a second or two, and
  reading the first one reads a value about to change again.
* **Back off on failure** — an accessory out of range does not become reachable by being
  asked more often, and without backoff one absent plug holds the radio for as long as it is
  away. Doubling from 4s, capped at 5 minutes so an hour-long absence still gets rechecked.
* **A budget** — 12 background reads a rolling minute. Past it rows stay stale and the
  operator refreshes by hand, which is the behaviour before any of this existed.

A failed read spends budget too: not counting it would let an unreachable accessory retry
without limit, which is the case the budget is for. And every reason for doing nothing is
NAMED (`Idle::RadioBusy`, `Settling`, `BackingOff`, `BudgetSpent`, `InFlight`,
`NothingStale`) — from outside, "nothing is stale" and "the radio is busy" look identical
and mean opposite things when something seems stuck.

The loop that had to be closed in the app: read outcomes are fed back to the scheduler from
the event drain, BEFORE the hub consumes them. Without that the backoff never engages — the
accessory stays stale, is picked again next tick, and holds the radio.

Pure: every decision takes the clock as an argument. No timers, no radio, no I/O.

---

## STILL TO DO — switching a Wemo plug on and off (2026-09-08)

Asked directly: *"keep enabling homekit wemo plugs on and off via BLE"*. Reading is wired —
adverts, GSN change detection, pairing, and the refresh scheduler above. WRITING is not, and
the gap is one specific piece.

### The blocker: the Broadcast Encryption Key

A HomeKit BLE characteristic write is encrypted with a session key derived after pairing.
`hap-core` parses adverts and tracks GSN; it does not derive that key, and the derivation
(HKDF over the pairing result, with the nonce discipline that goes with it) needs the spec
open beside it. Guessing at an HKDF salt produces writes the accessory silently ignores,
which is indistinguishable from a plug that is out of range — so this is not a thing to
approximate.

### The order to do it in

- [ ] Derive the Broadcast Encryption Key from the pairing, with the spec open. Test against
      a known vector before touching a radio.
- [ ] Nonce handling: the GSN is part of it, and `GsnTracker` already has the number.
      Reusing a nonce is worse than failing to write.
- [ ] One characteristic write, to the on/off characteristic of a paired plug.
- [ ] Feed the result back through `Refresher` the way reads already are — a write that
      changes state should mark the row fresh, not leave it stale until the next poll.
- [ ] Only then: the UI affordance. A toggle that writes optimistically and cannot tell
      whether the write landed is worse than a row that says "changed at the plug — refresh
      to read it", which is what exists today.

### Why the read path is the right foundation

`Event::AccessoryChanged` already marks a row stale when the plug is switched at the wall.
Once writes land, the SAME signal confirms one worked — the accessory bumps its GSN and
advertises, which the tracker already notices. Nothing new is needed on the read side.
