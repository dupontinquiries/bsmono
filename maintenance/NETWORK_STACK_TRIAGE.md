# The lean Rust network stack — triage, measured

Nine networking crates exist. Counted lines and *actual* dependants rather than
guessing which matter:

| crate | lines | dependants | verdict |
|---|---:|---:|---|
| `shared/snapcore` | 2,904 | **8** | **hub** — keep |
| `games/shared/netcode` | 2,556 | **6** | keep — the game layer over snapcore |
| `apps/.../snapclone-dht` | 1,388 | **0** | **77% duplicated** — see below |
| `apps/.../snapclone-microdht` | 899 | 2 | **retired in this commit** |
| `shared/dht-kit` | 881 | 1 | keep — protocol, snapcore uses it |
| `shared/net-kit` | 840 | 3 | keep — leaf primitives |
| `apps/.../snapclone-nat` | 567 | 4 | keep for now, see below |
| `shared/nat-kit` | 476 | **0** | orphan — should replace snapclone-nat |
| `shared/wake-kit` | 338 | 1 | keep — battery policy |

## Retired: `snapclone-microdht`

The first attempt at porting OpenDHT to Rust — an API boundary with a quick
local-net implementation, not a network-compatible DHT. Its only dependant was
one integration test, and that test now has a snapcore-backed twin that proves
the shared engine carries the same traffic. Both passed side by side for several
commits before this removal, which is what made it safe.

**899 lines and a whole crate, for a round trip snapcore does with no socket and
no thread.** `snapclone-group`: 99 unit tests and the round trip, still green.

## The big one: `snapclone-dht` has zero dependants

1,388 lines, and **nothing in the repo depends on it** — only workspace
membership and doc references. Broken down by module:

| duplicated (1,062 lines) | unique to snapclone (297 lines) |
|---|---|
| `wire` 352 → `dht-kit` was modelled on it | `identity` 147 |
| `engine` 357 → snapcore node/lookup | `presence` 71 |
| `core` 194 → snapcore routing/store | `gossip` 39 |
| `sim` 159 → `snapcore::sim` | `announce` 38, `resolver` 2 |

**77% of it is a second implementation of what snapcore and dht-kit already
do**, and the LAN parity harness has been demonstrating for several commits that
the two engines agree.

The lean move is to keep the 297 unique lines as a thin `snapclone-presence`
crate over `snapcore` + `dht-kit`, and drop the rest. That is a real refactor —
it moves the parity harness and the wire tests to new homes — so it is written
down rather than done in the same commit as a removal.

## `nat-kit` is an orphan and `snapclone-nat` is its consumer-in-waiting

`nat-kit` has zero dependants; `snapclone-nat` has four and pulls **tokio,
anyhow, bytes, rand and tracing** for a packet codec. Repointing the codec at
the kit leaves `snapclone-nat` as the async transport around it, which is what
it should have been. Blocked on the same thing as everything else here: the
change is easy, the verification needs an app build.

## Target shape

```
protocol   dht-kit ......... OpenDHT messaging      (no deps)
           nat-kit ......... STUN messaging          (no deps)
engine     snapcore ........ routing, store, lookup, punch, guard, sim
policy     wake-kit ........ when to spend the radio (no deps)
           net-kit ......... auth, http, json, framing
apps       netcode ......... game identity + sessions
           snapclone-presence  presence, gossip, identity  ← to extract
transport  snapclone-nat ... the async socket layer
```

Seven crates instead of nine, one engine instead of three, and every protocol
layer dependency-free.
