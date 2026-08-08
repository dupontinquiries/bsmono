# unendlich r746 drop — no bump needed, and bumping would have LOST work

Asked to check whether the monorepo is up to date with
`unendlich-r746-recipe-build-itemid-fix.zip` and bump it if needed.

**It is up to date. The monorepo is strictly ahead, and overwriting it with the
drop would have deleted the fleet integration.**

## Same version string, different trees

Both declare `unendlich-engine 0.99.9999996-r746-recipe-build-itemid-fix`. The
version matching is exactly why this needed checking rather than assuming — a
matching version is not a matching tree, and here the difference is substantial.

## What only the monorepo has

| | |
|---|---|
| `src/app/multiplayer_svc.rs` | fleet online play — the bsgames handoff + host/join control plane |
| `src/sim/netview.rs`, `src/bin/unendlich-netview.rs` | the netview tool |
| `src/world/worldpack.rs`, `terrain_llm.rs`, `worldpacks/` | world packs |
| `src/pngutil.rs` | |
| `docs/ART_TARGETS_OMITTED.txt` | |
| `crates/unendlich-engine/Cargo.toml` | the `netcode = { path = "../../../shared/netcode" }` dependency |

That last line is the one that matters most: **unendlich is wired into
`games/shared/netcode`**, the fleet's identity/discovery control plane, and the
drop predates that. Replacing the tree would have silently unwired it — the
build would still succeed, because removing a dependency and its only consumer
compiles fine.

## What only the drop has

One line, in `crates/unendlich-ui/src/lib.rs`:

```
-    egui::Frame::none()      <- the drop
+    egui::Frame::new()       <- the monorepo
```

`Frame::none()` is the *older* egui API. The monorepo already carries the
migration, so the drop is behind here too.

Everything else differing is build output: `.spv` shader binaries and
`Cargo.lock`. The monorepo additionally has two `.spv` files the drop lacks
(`clouds`, `double`).

## Verified

`cargo test -p unendlich-engine --lib` on the monorepo tree: **0 compile errors**,
3,737 tests reached the running stage before the container's disk filled at
100%. So the tree as it stands builds; the run was cut short by the environment
rather than by a failure, and the suite is NOT recorded as green.

## The rule this is an instance of

A drop with a matching version can still be older than the tree. Diff before
replacing, and diff for what the TARGET has that the source lacks — the direction
people forget, and the one that loses work.
