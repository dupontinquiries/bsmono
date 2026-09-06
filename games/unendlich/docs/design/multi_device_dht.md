# Multi-device mode over the DHT

One player, several devices: a **render box** runs the world, and any number of
**phones** (pure input surfaces: touch, IMU, voice, buttons) or **second screens**
join it, each holding a player slot. The phone-controller design
(`phone_controller_lan-v1r2`, §2b) says how they find each other without an IP:
a six-character **session code** shown on the render box's screen, resolved
through the DHT. This note is what of that is built, where the seams are, and
what the bin still owes.

## Built and tested — `sim::net::device_session`

| piece | what it is | tests |
|---|---|---|
| `SessionCode` | six chars from an alphabet with no 0/O/1/I/L; `from_seed`, `parse` (forgives case + separators, refuses what cannot be right), `grouped` `ABC-DEF` | 3 |
| `session_key` / `device_mailbox_key` | `H("unendlich-session:CODE")` for the advert record, `H("unendlich-devices:CODE")` for join requests — distinct from each other and from the world-join mailbox, because a mailbox `get` drains | 1 |
| `DeviceCaps` | the `X-Device-Capabilities: touch,imu,voice,buttons` header plus `display`; `is_controller` = no display | 1 |
| `DeviceAdvert` | what the render box publishes: host id, name, world, used/max slots, last-seen. **No address.** Wire codec, garbage-tolerant | 2 |
| `DeviceSession` (host) | code, advert on cadence (`announce`), `close` (publishes full), `pump` the device mailbox → admit / reconnect / refuse with `Accept`/`Deny` into the device's own mailbox; slots over `player_roster::Roster` (slot 0 = the render box); names deduplicated `Phone`, `Phone (2)`; `heartbeat` / `prune` (stale → disconnected keeps slot → dropped releases it); `kick`; `rows` / `view` for the menu; `qr_payload` | 6 |
| `DiscoveryLadder` (phone) | mDNS → address cache → DHT → manual; a hit short-circuits, a miss moves on at once, a timeout moves on, a late answer for a step already left is ignored, which step succeeded is remembered | 2 |
| `AddressCache` | last four addresses that worked, most recent first, file round-trip | 1 |
| `DeviceJoin` (phone) | request into the device mailbox, poll own mailbox, re-send after 3 s, give up after 3 attempts | 2 |
| `DevicesView` | what the Devices screen shows; one status line for hosting / searching / idle | 1 |

`MailMsg::DeviceRequest { from, name, caps }` is a new tag (4) in the existing
`dht_mailbox` codec; older decoders return `None` for it and ignore it.

## The screen — `app::live_menu`

Screen kind **5**. On a render box: the code at 34 pt, the QR payload, the
device table (slot · name · what it is · liveness dot · kick), Stop Hosting.
On a phone: which discovery step is running or succeeded. Idle: Start Hosting.
Two new `menu_pick` codes, not renumbering the others: **10** Devices /
Start Hosting, **11** Stop Hosting; `LiveMenuOut::device_kick` is the slot.
The Multiplayer screen has a Devices button and the session's status line.
Drawn headless in the crate's tests like every other screen.

## The IP-protection invariant is kept

The design stores the render box's `IP:port` in the DHT value. `dht_mailbox`
already decided that an address travels only in an `Accept` to an admitted
peer, and a session code is read across a room — not a secret. So the advert
carries id + name + free slots, the device asks through the mailbox, and the
address comes back only to a device the host admitted. Same rule as world join.

## What the bin owes (the on-device step)

1. **Two DHT primitives on a real node.** `SessionRecords` (put-value /
   find-value, last-write-wins) for the advert; the existing `MailboxDht` for
   requests and answers. `LocalRecords` / `LocalDht` are the in-process
   stand-ins the tests run on; `mp_dht_mailbox` shows how the mailbox half
   maps onto the Kademlia model.
2. **The host loop**: construct `DeviceSession` with the socket address, then
   per tick `announce`, `pump`, `heartbeat` on every input packet, `prune`,
   and hand `view(now)` to the menu (`LiveMenuState::devices`, currently
   `None` at the one construction site in `world_renderer`). Route pick 10/11
   and `device_kick`.
3. **The phone loop**: `DiscoveryLadder` with the real mDNS query, cache
   probes, DHT lookup and QR scan feeding `report`; on an advert, `DeviceJoin`;
   on `Paired { addr }`, the WebSocket/UDP connect the design describes.
4. **Splitscreen**: one viewport per controller-only device — untouched here.
