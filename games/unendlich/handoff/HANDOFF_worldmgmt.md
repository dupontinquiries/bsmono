# HANDOFF — wiring `sim::world_management` (WID r335-worldmgmt)

**Status:** PROPOSAL ONLY. The pure core (settings + the Open-to-DHT transition
decision) is done + verified (9/9). The settings page UI, the NAT candidate
discovery, and the DHT bootstrap/announce loop are bin/IO glue (SHARED_RULES
§1–§2) — coordinator/operator wire on device. This is the gate to the
real-DHT-node work in `NEXT_SESSION_GUIDE §3c`.

## Division of labour
- **Pure (this module):** the `WorldMgmtSettings` model (validated), and
  `apply_open_to_dht(settings, self_id, role, candidates, now_ms) -> HostTransition`
  which decides *what* the toggle implies and **builds the `WorldAdvert`** from
  the settings via `session_mode::HostFlow::make_advert`.
- **Glue (bin):** render the page; gather the host's NAT `candidates` and the
  wall-clock `now_ms`; on `GoHost`, bootstrap the DHT node and announce the
  advert; on `CloseHost`, stop announcing and tear the host down.

## Why `apply_open_to_dht` takes `candidates` + `now_ms`
`make_advert` needs the host's reachable endpoints (NAT discovery — IO) and a
timestamp (wall clock). SHARED_RULES §1 forbids IO/clock in the pure core, so
both are **passed in** by the bin. The module stays pure and deterministic; the
brief's 3-arg sketch is realized as 5 args for exactly these two bin-owned inputs.

## Proposed: the World-Management settings page
A page with four rows bound to the validated setters:

| Row            | Control      | Setter                         | apply_kind        |
|----------------|--------------|--------------------------------|-------------------|
| Open to DHT    | toggle       | `set_open_to_dht(bool)`        | SessionLifecycle  |
| Whitelist      | toggle (on)  | `set_whitelist_on(bool)`       | Live              |
| Max Players    | stepper 1..16| `set_max_players(u8)` (clamps) | Live              |
| World Name     | text field   | `set_world_name(&str)` (→bool) | Live              |

`apply_kind(field)` tells the page which rows hot-apply (Whitelist/MaxPlayers/
Name) vs which one changes the session lifecycle (Open to DHT). Host this page as
a submenu under the pause/world menu (T-menus owns the submenu tree; expose this
page as one of its entries).

## Proposed: the Open-to-DHT handler (the lifecycle step)
When the Open-to-DHT toggle changes, call `apply_open_to_dht` and act on the
transition. `self_id` is the local player's `UnendlichId`; `role` is the running
`SessionRole`:

```rust
let candidates = nat_discover_candidates();        // bin: STUN/UPnP/LAN reflexive
let now_ms = wall_clock_ms();                       // bin: clock
match sim::world_management::apply_open_to_dht(&settings, &self_id, role, candidates, now_ms) {
    HostTransition::GoHost(advert) => {
        // NEXT_SESSION_GUIDE §3c — the real-DHT-node step:
        dht.bootstrap();                            // join the DHT overlay
        dht.announce(&advert);                      // publish under advert.key()
        session_role = SessionRole::Host;           // flip the world loop's role
        host_flow.open(); host_flow.go_live();      // session_mode host FSM
        // re-announce on the §6 cadence (ADVERT_REANNOUNCE_MS); refresh
        // advert.last_seen_ms / players as the world changes.
    }
    HostTransition::CloseHost => {
        dht.stop_announce(&advert_key);             // stop publishing
        dht.leave();
        session_role = SessionRole::Local;          // back to single-player
        host_flow.close();
    }
    HostTransition::StayLocal => { /* no lifecycle change */ }
}
```

Whitelist enforcement on join is already `net::identity::{Whitelist, Mailbox}` —
when hosting, gate incoming `JoinRequest`s by `settings.whitelist_on` (whitelisted
ids auto-admit; others land in the mailbox for host approval).

## Channels / cadence it consumes
On `GoHost`, the host opens the §6 channels (`sim::net_channels::DEFAULT_CHANNELS`)
and re-announces on `ADVERT_REANNOUNCE_MS`; `chunk_stream`/`entity_snapshot` carry
world state to joiners. That MP IO continues from here (coordinator-owned).

## Verify after wiring (on device)
- Single-player world: Open-to-DHT off ⇒ no DHT traffic. Flip it on ⇒ the world
  appears in another instance's Server Browser (advert published under
  `(host_username, world_name)`), a whitelisted peer can join.
- Flip Open-to-DHT off while hosting ⇒ announcing stops, the world returns to a
  local session; flipping the toggle while already hosting is a no-op.
- Max Players caps the lobby; renaming updates the advert's `world_name`.
