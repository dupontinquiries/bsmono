# Why the Wemo WSP100 can't be switched yet

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
