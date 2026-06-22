# Testing

## How to run

```bash
# All tests
cargo test

# With the service feature on (covers extra IPC tests)
cargo test --features "qt service"

# With everything
cargo test --features full

# A specific test
cargo test --features "qt service" test_normalize_thermal_reading
```

## Coverage by module

### Solid

- **`screen_temps.rs`** — 11 tests. Covers binary format round-trip,
  Gaussian kernel correctness, ambient baseline, frame parsing
  errors, atomic publish, rate-limiting, σ-helper behavior across
  sensor counts.
- **`screen_temp_hints.rs`** — 13 tests. Covers hint file parsing,
  multi-file max merge, stale expiry, malformed JSON skip,
  resolution chooser across N=1, 4, 10, 53 (matched + mismatched
  σ), and clamp behavior.
- **`thermal_reading_tests`** in main.rs — 12 tests. Covers
  millidegrees standard path, microdegrees fallback (the 900 °C
  bug), raw-degrees ambiguity, extreme garbage clamping, plus
  the new `detect_unit_divisor` / `apply_unit_divisor` helpers
  (boundary temperatures, round-trip, garbage clamping, and the
  cache-prevents-reinterpretation property the per-sensor cache
  was added to fix).
- **`latest_database_tests`** in main.rs — 5 tests. Cover the
  newest-by-timestamp pick, pattern-matching filename filter,
  nonexistent dir, no-matching-files, and empty dir.
- **`systems::ShutdownSignal`** — 4 tests. Pinned-down invariants:
  starts unset, propagates across clones (the load-bearing
  Arc-sharing property), monotonic (no clear), and `Default`
  constructable.
- **`model_system.rs`** — 6 tests. Original three (predict
  happy-path, calibrations drained into models, empty-sensors
  bail) plus three new ones for the audit-log fields: source
  attribution preserved through `apply_calibrations`,
  `unix_when` populated with a sane wall-clock value, and the
  sample buffer respecting `CALIBRATION_SAMPLE_CAPACITY` with
  FIFO eviction.
- **`db_writer.rs`** — 4 tests. Happy-path batch commit,
  full-queue drop without blocking, savepoint rollback on a bad
  row in the middle of a batch, concurrent producers.
- **`service::map_pin`** — 5 tests. Validation rejection paths,
  size caps, well-known categorization, round-trip serialization.
- **`service::pin_store`** — 5 tests. Upsert+list, replacement,
  equivalent-upsert-doesn't-fire-change, expiry sweep, remove
  fires change.
- **`service::databus_client`** — 4 tests. Kind ↔ layer round-trip,
  custom kind passthrough, object payload ingestion, self-publish
  loop-prevention.
- **`service::inbox`** — 4 tests. File import, ID fallback to
  filename, malformed-JSON-doesn't-panic, invalid-pin-rejection.

### Thin

- **`temperature_field.rs`** (neural field) — no real unit tests.
  Hard to test in isolation; sensitive to weight initialization and
  ordering of updates. We mostly catch regressions at runtime.
- **`forecast.rs`** — no unit tests. Same issue as the neural field.
- **`world_state.rs`** — no tests. The `read()` / `write()` helpers
  could be tested for poison-recovery; not high priority.
- **`environment.rs`** — no unit tests for IIO sysfs parsing.
  Would need filesystem fakes; possible but not done.
- **`iio_proxy.rs`** — no unit tests. zbus mocking is non-trivial.

### Gaps we know about

- **No integration tests for D-Bus.** Spawning a service and calling
  it from a test is doable but adds a `tokio` test runner; we
  haven't done it. Manual `gdbus call` is the current verification.
- **No QML integration tests.** The Qt frontend is verified by
  running the app and looking at it. A real test rig would use
  `qmltest` and is out of scope.
- **No GTK integration tests.** Same situation.
- **`Database::create_consistent_backup`** doesn't have a dedicated
  test, though the import/export CLI paths exercise it. A
  standalone test would be 30 lines and useful.

## What "good test" means here

These are the criteria we use when deciding whether a piece of code
needs a unit test:

1. **Does it have a non-obvious correctness path?** (e.g., the
   thermal reading normalizer tries three divisors and picks one
   in range — there are five conceptual outcomes worth pinning
   down.)
2. **Does it have invariants that could regress silently?** (e.g.,
   the binary frame format. A future "let's just add a field"
   change would break consumers if the magic + version + offsets
   weren't checked in tests.)
3. **Is it on a code path that's hard to debug in production?**
   (e.g., the DB writer's savepoint-rollback — by the time you
   notice rows are missing, the batch is gone.)
4. **Would the test help future Claude understand the intent?**
   The 53-sensor resolution test isn't there because we worry
   about a regression; it's there because the math is non-obvious
   and the test pins down what we *meant*.

If none of these apply, skip the test. We're not chasing coverage
percentages.

## Test patterns used here

- **Unit tests in the same file as the impl** under `#[cfg(test)]
  mod tests { ... }`. Reduces churn when refactoring.
- **`tempfile::tempdir()`** for tests that touch the filesystem.
  Auto-cleanup, no test pollution.
- **Atomic counters** (`Arc<AtomicU32>`) when testing callbacks
  that need to count fires.
- **Worked-example tests** with documented expected outputs and
  ±1 tolerance for float rounding (e.g., the 53-sensor cases).

## What we don't do

- **No mocking frameworks.** If a function needs a mock to test, we
  consider it a sign that the function should take its dependencies
  as parameters.
- **No property-based testing (`proptest` etc.).** Considered for
  the thermal normalizer; the input space is small enough that
  hand-picked cases cover it.
- **No `tokio::test`.** Synchronous tests only. The async code is
  thin enough that we don't need a runtime to test it.

## Adding tests

When adding a feature, ask: "what's the worst silent-failure mode?"
That's the test. Write it before the implementation if you can,
after if you can't.

If a bug is reported, the fix lands with a test that would have
caught it. The test stays in the codebase as a tripwire.
