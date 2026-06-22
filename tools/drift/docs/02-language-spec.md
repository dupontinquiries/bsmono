# 02 — Language / surface spec

Status tags on every item: **[IMPLEMENTED]** = in `playground/`, compiles and
runs. **[SPEC]** = designed, not built. Do not assume [SPEC] items work.

## Zero-cost-when-off mechanism [IMPLEMENTED]

Drift is gated behind a cargo feature. The `#[cfg(feature = "drift-trace")]`
lives *inside* each macro expansion, so when the feature is off the recording
calls and the entire journal module are stripped before name resolution. The
normal build pays nothing and any deliberately-pure module stays pure.

```toml
[features]
drift-trace = []
```

```sh
cargo build                          # off: macros expand to nothing
cargo build --features drift-trace   # on:  journal active
```

## The boundary-journal macros [IMPLEMENTED]

Producer side — wraps a boundary-crossing function body, records that the
intent was serviced, returns the body value unchanged:

```rust
pub fn create_task(title: &str) -> u64 {
    intent!("create_task", {
        // ... real body ...
        1
    })
}
```

Consumer side — declares "I am about to invoke this intent across the
boundary," placed right before the call site:

```rust
fn on_add_clicked() {
    attempt!("create_task");
    backend::create_task("buy milk");
}
```

Reconciliation — prints the three-way diff; no-op when the feature is off:

```rust
drift_report!();
```

## `#[temporary(...)]` and the expiry hierarchy [SPEC]

Temporary code should carry an expiry the toolchain escalates over time
(warning → error). Prefer the strongest available form:

```rust
#[temporary(expires_when = exists("queue_delivery"))]  // BEST — can't be wrong;
                                                        // stale the moment the
                                                        // replacement exists
#[temporary(expires = "v1.8")]                          // OK   — slips with the release
#[temporary(expires = "2026-09-01")]                    // WEAKEST — a calendar guess
fn parse_legacy_blob() -> User { ... }
```

A successor-conditioned expiry is provably correct: when the named successor
artifact exists, the temporary thing is stale by definition. Time-based expiry
is the fallback when there's no concrete successor to point at.

## `#[intent(...)]` / `#[depends_on(...)]` — behavior contracts [SPEC]

Type contracts are the compiler's job. Drift adds *behavior* contracts at the
boundary: terminality and timing.

```rust
// producer
#[intent(name = "create_task", returns = terminal, latency = "<50ms")]
async fn create_task(...) -> TaskState { ... }

// consumer (e.g. a QML handler, conceptually)
#[depends_on(intent = "create_task", assumes = terminal)]
fn animate_new_task() { ... }
```

What's checkable (and what isn't): the magic was never magic. If the return
type is an enum whose variants are tagged terminal / non-terminal, then adding
a non-terminal variant that a consumer treats as terminal is a *structural*
drift (caught at build). Latency is a *temporal* drift (caught at runtime via
the journal). "Immediately" and "success-the-concept" are only tractable once
the state machine is a declared artifact — don't promise semantic understanding
beyond that.

## `cargo drift` subcommands [SPEC — NOT IMPLEMENTED]

The plugin shape that makes Drift "feel like a language" while being just Rust.
None of these exist yet; they are the build target.

```
cargo drift journal      # the two-sided reconciliation (the MVP, currently a
                         #   drift_report!() call — promote it to a subcommand)
cargo drift debt         # the accumulated acknowledged-drift ledger
cargo drift impact <fn>  # blast radius: which UI / modules this boundary touches
cargo drift check        # static boundary scan (e.g. Rust↔QML reachability)
cargo drift outline      # derived effect-summary view for fast reading (docs/04)
```

`cargo drift check` and the QML scan must run as a **standalone pass over
non-Rust files**, not as something injected into the crate graph — so it never
violates module purity.

## Type vocabulary [SPEC]

Real (have verifiers): `Temporary<T>`, `Deferred<T>`, `Legacy<T>`,
`Regrettable<T>` (= "rewritten N times", verifier: VCS churn count).

Rejected (no verifier — keep as comedy, not code): `Political<T>`, `Human<T>`,
`AncientEvil<T>` as a *type*. (As a debt *label* with a severity ordering,
"AncientEvil" is fine — that's a tag on a real ledger entry, not a type.)
