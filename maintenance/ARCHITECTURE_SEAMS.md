# Breaking up the big loops — measured, and it isn't what the phrase suggests

Asked whether there's a systematic way to decompose the big application loops.
I measured before answering, and the measurement changes the answer.

## The finding

Of **677 files over 300 lines** across `apps/` and `shared/`:

```
427 files (63%)   253,419 lines   ZERO real effect calls
```

No filesystem, no process, no socket, no clock, no thread. **They are already
pure logic.** They simply live in crates that also contain the few files that
do I/O.

And the loops are not tangled either. `voiceterm/src/main.rs` is 23,388 lines
with **140 effect calls — a density of 0.6%**. Effects are already concentrated.

## So the problem is not the loops. It's that the crate boundary doesn't follow the effect boundary.

That single sentence is the systematic answer. The decomposition work has
largely been done, file by file, by whoever wrote each module. What was never
done is drawing crate lines along the seam that already exists — so:

* 133k lines compile as **one serial frontend pass** (Rust's frontend is
  single-threaded per crate)
* pure logic can't be tested headless, because its crate needs a GPU or a socket
* nothing can be reused, because depending on the logic means depending on the app

## Why "count imports" gives the wrong answer

My first measure counted `use std::net::SocketAddr` as I/O and reported
`snapcore::routing` — a pure routing table — as effectful. **A type names a
domain; it does not perform anything.** Counting imports finds files that
*mention* effects; counting calls finds files that *have* them, and only the
second tells you where a boundary can go.

`tools/astfind/src/purity.rs` implements the corrected measure so it can be
re-run rather than re-argued.

## The four moves, in the order they pay

Each is something this repo already does somewhere, which is why they're
credible rather than aspirational:

1. **Take `dt_ms`, don't read a clock.** `Instant::now()` is an effect; holding
   an `Instant` is not. Every ported widget here — `HoldTracker`,
   `ChevronConfirm`, `DialWatermark`, `AmbientBlobs` — moved across the purity
   line on this one change, and each exposed a bug it had been hiding: resumed
   suspends, backwards clocks, unbounded phase accumulation.
2. **Inject the effect as a closure.** `astfind`'s `exists`, `cleanup-kit`'s
   `Engine`, the gitmind browser's `lister`. Tests then need no filesystem, no
   model and no SSH host respectively.
3. **Return a decision, don't perform it.** `cleanup-kit` returns a `Proposal`
   the user accepts; `camera_select` returns a `Selection` the shell opens;
   `wake-kit` returns a `Plan`. The caller owns the effect, so the policy is
   testable and the effect is one place.
4. **Then move the pure files into their own crate**, and only then. Doing this
   first produces a crate that still needs a socket.

## What I could not measure

`nproc` in this container is 1, so I cannot tell you how much wall-clock a split
actually buys on your hardware. The serial-frontend argument is structural and
holds regardless, but **the size of the win needs `cargo build --timings` on a
real machine** before anyone spends a week on the refactor.
