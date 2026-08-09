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

---

# snapcore — synced, and it was carrying a memory-exhaustion hole

`touchkit` turned out identical in both games; no action. `snapcore` did not.

The shared crate is a strict superset — 289 lines of `store.rs` against the
vendored 168, with nothing unique on the vendored side — and the difference is
not cosmetic. `StoreLimits` gained two caps the vendored copy lacks, and the
comment explains what they are for:

> Total distinct KEYS retained. The per-key cap bounds how much one key can
> hold; nothing bounded how many keys existed, so a client could exhaust a
> node's memory by PUTting one value under each of a million keys — the per-key
> limit never fires because every key holds exactly one value.

So skydribble and blackstar-rs have each been shipping a DHT store that any peer
can OOM, weeks after the fix landed in `shared/snapcore`. That is the vendoring
cost made concrete: not untidiness, an unpatched node.

## Why this one was safe to sync when netcode's repoint was not

Checked before touching anything:

* **Neither game references `snapcore::` directly.** It reaches them only
  through `netcode`, so no game code can break on an API change.
* **Nothing constructs `StoreLimits` explicitly** in either netcode copy — it is
  only used through `Default`, and the new `Default` fills the added fields.
* The copy is a **file sync, not a dependency change**: the build graph is
  untouched, so the thing I could not verify for the repoint does not arise.

Verified after: the synced crate's own 50 tests pass, and the vendored `netcode`
that depends on it still compiles.

## What remains

`netcode`'s vendored copies are still behind by `attest`, `lobby` and
`tournament`. Those are additive, so syncing them looks equally safe — but
`netcode` is the crate the games *do* call directly, and the last check that
matters (does the game still build) cannot run here without `libasound2-dev`.
