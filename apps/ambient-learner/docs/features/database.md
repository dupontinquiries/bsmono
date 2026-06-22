# Database

## Files

```
weather_data/
├── sensors_20260512_102240.db        # active database
├── sensors_20260512_102240.db.backup # snapshot taken at last open
├── sensors_20260505_180001.db        # older, kept for recovery
└── ...
```

Discovery: `get_latest_database()` scans `weather_data/` for files
matching `sensors_<YYYYMMDD_HHMMSS>.db`, sorts by timestamp,
returns the newest. New files are created with the current timestamp
when no prior DB exists.

## Schema

```sql
CREATE TABLE readings (
    timestamp     INTEGER PRIMARY KEY,
    data          TEXT,                 -- JSON-encoded SensorReading
    location_id   TEXT                  -- optional; added via migration
);

CREATE TABLE calibrations (
    timestamp     INTEGER PRIMARY KEY,
    sensors       TEXT,                 -- JSON-encoded [f32]
    target        REAL,                 -- the ground-truth temp the user entered
    location_id   TEXT                  -- optional; added via migration
);
```

The `data` and `sensors` JSON columns deliberately don't have a
formal schema — they're versioned by the contents, which is fine
for our purposes. If we ever add a third version of `SensorReading`
that's incompatible we'll bump a schema cookie.

## Migrations

Migrations are run by `Database::ensure_column` on every open. The
function queries `PRAGMA table_info(<table>)` and only runs the
`ALTER TABLE ADD COLUMN` if the column doesn't already exist.

This is **idempotent and quiet**. Previous versions ran the alter
unconditionally inside `let _ = ...` — that "worked" by ignoring
duplicate-column errors, but it dirtied the WAL on every startup
and made schema diffs harder to read.

To add a new migration:

1. Add a new `Self::ensure_column(...)` call in `Database::open()`.
2. Update the schema documentation above.
3. Confirm the new column is nullable (NOT NULL with no default
   would break old DBs).

## Pragmas

Set on every connection (both `new()` and `open()`):

```
journal_mode = WAL
synchronous  = NORMAL
cache_size   = 10000
busy_timeout = 2000ms
```

The 2-second busy timeout means SQLite internally retries on
`BUSY` for up to 2s before returning an error. Combined with our
explicit retry loops, contention essentially never surfaces to
callers in normal operation.

`temp_store=MEMORY` is also set in the DB writer's connection
(slightly faster on systems where `/tmp` is on slower storage).

## Backup / consistency

**On open:** `Database::create_consistent_backup` snapshots the
source DB to `<path>.backup` using SQLite's online backup API
(`Backup::new(...).step(-1)` in a loop). This is the only safe
way to copy a SQLite file with active `-wal` and `-journal`
sidecars.

**Why not `std::fs::copy`?** The bare file copy doesn't include
the sidecars, so a backup taken right after a crash can be
missing recently-committed transactions or be partially applied.

**Why not `run_to_completion`?** Because `run_to_completion(-1, ...)`
panics on rusqlite 0.31 — it requires a positive page count. Use
`step(-1)` in a loop instead.

## The async writer

See `src/db_writer.rs` for the implementation and
`docs/03_DESIGN_DECISIONS.md::ADR-004` for the rationale.

Hot-path writes go through `DbWriter::submit_reading(&reading,
location_id)` which is non-blocking (`mpsc::sync_channel(1024)`,
returns `false` on full). A dedicated `ambient-db-writer` thread
drains the channel every 250ms or whenever 256 entries accumulate,
whichever comes first, and commits everything as a single
`BEGIN IMMEDIATE / INSERT × N / COMMIT` transaction.

Each row inside the batch uses a savepoint, so a single bad row
(e.g., primary key collision) doesn't abort the rest of the batch.

## Synchronous fallback

The original sync paths (`db.store_reading(&reading, location_id)`,
`db.store_calibration(...)`) still exist on the `Database` impl
and are still used by the GTK UI and the calibration path. They
now use:

- Prepared cached statements (saves the SQL parse cost per call)
- Exponential backoff with jitter on `SQLITE_BUSY`

Migrating the remaining sync callers to the async writer is on
the TODO. Calibrations are kept synchronous deliberately —
they're rare and durability-critical.

## Import / export

```bash
# Import an external DB into our data dir as the new "latest"
ambient-learner --import-db /path/to/external.db

# Export the current "latest" to an arbitrary path
ambient-learner --export-db /tmp/snapshot.db
```

Both use the online backup API for safety. Import runs
`PRAGMA integrity_check` on the source before copying; the import
fails if the source is corrupt. Destination paths inside the data
dir use a fresh timestamp (`sensors_<now>.db`) so the imported DB
becomes the new "latest" on next launch.

The `--db <PATH>` flag for in-place override is declared in `Args`
but **not yet plumbed** through the GTK / Qt code paths. See TODO.

## Performance notes

A 100ms sensor poll on a 53-sensor device generates 53 readings/tick
= 530 rows/sec. Without batching, that's 530 `fdatasync()` calls
per second — easily 30-50ms of disk wait per second on a phone's
slower flash. With the batched writer, those 530 rows commit as
~2 transactions/sec, each with one `fdatasync()`.

Empirically observed (desktop SSD, single-writer WAL):
- Sync writes: ~150-200 rows/sec sustained
- Batched writes: ~10,000-15,000 rows/sec sustained

The bottleneck shifts from disk to SQLite's row insertion cost,
which is negligible.

## Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `database is locked` | Two writers on same file | Ensure only one `DbWriter` per DB |
| `pages_per_step must be positive` | Old `run_to_completion(-1, ...)` | Use `step(-1)` loop (fixed) |
| `disk I/O error` on commit | Out of disk space | Check `df`; rotation TODO |
| Integrity check fails on import | Source DB corrupt | Restore from `<src>.backup` |
| Slow queries on large `readings` | No index on `timestamp` | PK is `timestamp`, queries should use it |

## What's missing

- **No automatic rotation.** The data dir accumulates `sensors_*.db`
  files forever. A scheduled prune of files older than N days would
  help but isn't implemented.
- **No vacuum schedule.** `Database::vacuum_database()` exists but
  isn't called automatically. Manual via `dbmaint` if needed.
- **No connection pool.** One writer, sometimes one reader. We
  could open one read-only connection per UI tick instead of
  reusing the writer's connection for reads — would help a bit but
  isn't hot.
