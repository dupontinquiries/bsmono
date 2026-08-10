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

---

## Correction: I wired the weaker scheduler

`snapcore::schedule` had **eleven tests and no caller** — a policy nothing
consulted. Meanwhile I wired `wake-kit`'s gating into `snapclone-dht`'s engine,
which is the *worse* of the two for that job:

| | knows about |
|---|---|
| `wake-kit` | Interactive / Background / Opportunistic across arbitrary app work |
| `snapcore::schedule` | **which bucket** went stale, **which values** are durable, and whether the device is on battery |

Both are right for their scope. Using the general one for the DHT-specific job
was not, and it happened because I did not look for an existing scheduler before
reaching for the kit I had just built.

Now wired: `Node::next_maintenance` drives `schedule::plan`, and the routing
table gained the one input the scheduler asked for and the table never kept —
**per-bucket last-query times**. That absence is why `plan` could not have had a
caller: it wanted a number nobody was recording.

`note_query` is called from `handle_find_node` and `handle_find_value`, so
**ordinary traffic keeps buckets fresh and maintenance only covers what traffic
missed**. That is the whole advantage of measuring staleness over refreshing on
a timer, and it is now tested: a just-queried bucket plans nothing.

snapcore 71 tests, 82 with the wire feature; netcode's 86 still pass.

`wake-kit` keeps its place — cross-app periodic work is a real problem and the
apps' 1s/2s/5s/20s/60s/360s timers are still unbatched. What changes is that the
DHT does not use it, because something better was already there.
