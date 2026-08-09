# The vendored crates have drifted in both directions

`games/skydribble` and `games/blackstar-rs` each vendor `netcode`, `snapcore`
and `touchkit` under their own `vendor/`, depending on them by path:

```toml
netcode  = { path = "vendor/netcode" }
touchkit = { path = "vendor/touchkit" }
```

Those crates also exist in this monorepo — `games/shared/netcode`,
`shared/snapcore` — and the copies are **not** in sync. The drift runs both
ways, which is what makes it worth writing down rather than simply repointing:

| | has that the other lacks |
|---|---|
| `games/shared/netcode` | `attest.rs`, `lobby.rs`, `tournament.rs` |
| `vendor/netcode` | `Multiplayer::reannounce()` |
| `shared/snapcore` | `punch.rs`, `schedule.rs` |

## `reannounce` — ported upstream

The vendored copy carried a real fix the shared crate did not: records and
presence carry TTLs on the store, so **a host that announces once and plays for
ten minutes silently falls off the friend list**. The session is alive and
undiscoverable, which to everyone else looks like the host left.

It is now in `games/shared/netcode`, with three tests it never had in the
vendored copy — including the ordering case that matters, a timer firing once
more after `stop_hosting` and correctly *not* resurrecting the session.

## Why the vendor directories were not simply repointed

`netcode`'s shared version is now a strict superset for the two modules
skydribble actually uses (`handoff`, `multiplayer`), so repointing looks safe.
It was not done here for one reason: **the games do not build in this
container** — both need `libasound2-dev` to link — so a dependency change could
be made but not verified, and an unverified build-graph change to two games is
worse than a documented divergence.

`snapcore` is a separate question: its shared version has `punch.rs` and
`schedule.rs` that the vendored copy lacks, and nobody has checked whether the
games' code compiles against the newer API.

## The rule this is an instance of

Vendoring a crate that lives in the same repository buys nothing and costs
synchronisation. Every fix then has to be made twice, and — as here — sometimes
is not: `reannounce` sat in two game trees for weeks while every other consumer
of `netcode` kept dropping off friend lists.
