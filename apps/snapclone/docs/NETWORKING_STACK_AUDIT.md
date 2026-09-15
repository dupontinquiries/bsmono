# The Rust networking stack: what is wired, and what is not

Audited 2026-09-15 against the tree, prompted by "make sure the Rust
networking stack is complete". It is not incomplete in the way that phrase
usually means — almost nothing is a stub. **41 crates, ~34,000 lines, 700+
tests, and every one of them is a workspace member, so every one compiles and
is tested by `cargo test` in `rust/`.**

The problem is the other kind of incomplete.

## 18 of 41 crates are reachable from nothing

Neither linked by CMake (the Qt app's route into Rust, via the 21 crates that
expose a C ABI) nor named as a dependency by any other crate or by
`egui-frontend`:

| lines | kind | crate |
|---:|---|---|
| 1349 | rlib | `snapclone-videosrc` |
| 1223 | rlib | `snapclone-databus` |
| 788 | C-ABI | `snapclone-gstpipeline` |
| 730 | rlib | `snapclone-landmark` |
| 668 | rlib | `snapclone-piggyback` |
| 652 | rlib | `snapclone-ui-traits` |
| 454 | C-ABI | `snapclone-proto` |
| 449 | C-ABI | `snapclone-wirecodec` |
| 398 | rlib | `snapclone-firewall` |
| 366 | rlib | `snapclone-upnp` |
| 351 | rlib | `snapclone-debounce` |
| 338 | rlib | `snapclone-vcard` |
| 329 | rlib | `snapclone-gossip` |
| 280 | rlib | `snapclone-relay` |
| 270 | rlib | `snapclone-session` |
| 268 | C-ABI | `snapclone-gesture` |
| 199 | rlib | `snapclone-network-diag` |
| 171 | C-ABI | `snapclone-linkpreview` |

That is roughly 7,300 lines that build, pass their tests, and cannot affect
the running application.

## Why this is worth writing down rather than fixing

Four of them expose a **C ABI** — `gstpipeline`, `proto`, `wirecodec`,
`gesture`. A crate compiled as a `staticlib` that nothing links is
unambiguous: it was built to be called from the Qt side and the call never
landed. Those four are the strongest candidates for "finish the wiring".

The rest are `rlib`s, and there the reading is genuinely ambiguous from the
tree alone. `snapclone-relay`, `snapclone-upnp` and `snapclone-nat` are the
NAT-traversal trio; `nat` IS reached and the other two are not, which looks
like one path of three being live rather than three dead crates.
`snapclone-gossip` and `snapclone-presence` are the same shape — presence is
wired, gossip is not, and `egui-frontend/src/gossip_wire.rs` exists, so the
frontend may be carrying its own copy of what the crate does.

Deciding which is which needs someone who knows what was intended. A survey
can say "this is not connected"; it cannot say whether that is a gap or a
retired approach, and guessing would turn an honest inventory into a wrong
to-do list — which is exactly what `PORTING_PARITY.md` had become.

## What the audit does establish

* **Nothing is missing for want of code.** The crates with the most substance
  — `audio` (5435 lines, 104 tests), `group` (5190/100), `face` (2596/40),
  `nat` (1653/18), `mediashare` (1552/27) — are all real and all reached.
* **Three crates have zero tests**: `snapclone-storage` (371 lines),
  `snapclone-core` (313), `snapclone-network-diag` (199). The first two ARE
  reachable, which makes them the more interesting gap: live code with no
  coverage beats dead code with none.
* **`snapclone-identity` is the hub** — five consumers, and at 68 lines the
  smallest crate in the tree. That is a good sign, not a bad one: the thing
  everyone depends on should be small.

## Suggested order, if this gets picked up

1. The three untested-but-live crates: `storage`, `core`. Cheapest real risk
   reduction in the stack.
2. The four unlinked C-ABI crates, which are unambiguous unfinished wiring.
3. The `rlib` set, one judgement call at a time, starting with the pairs where
   a sibling IS wired (`relay`/`upnp` against `nat`, `gossip` against
   `presence`) — those comparisons are the ones a reader can actually settle.

## Method

`Cargo.toml` `members` for compilation, `[dependencies]` across every
`Cargo.toml` under `rust/` and `egui-frontend/` for Cargo reachability, and a
grep of `CMakeLists.txt` plus `cmake/` for the C++ link route. A crate counts
as reached if either finds it. Re-runnable by hand; nothing here is cached.
