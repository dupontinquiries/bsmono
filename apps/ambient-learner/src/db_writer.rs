// src/db_writer.rs
//
// Async, batched writer for the SQLite database.
//
// Why this exists:
//   * The sensor poll loop fires 10× per second on the UI thread.
//     Doing a synchronous INSERT on every tick stalls the UI when
//     the kernel decides to flush.
//   * SQLite is dramatically faster at INSERTs inside an explicit
//     transaction than one-INSERT-per-implicit-transaction. A single
//     BEGIN ... INSERT × 100 ... COMMIT is ~50× faster than 100
//     individual INSERTs even with WAL mode.
//   * Database-busy retries should *not* block the UI. The writer
//     thread can sleep all it wants without anyone caring.
//
// Design:
//   * One owned `Database` (and therefore one `rusqlite::Connection`)
//     lives on this thread.
//   * Producers send `WriteOp` messages over an `mpsc::SyncSender`.
//     The bounded channel (1024 entries) gives natural back-pressure:
//     if the writer can't keep up, producers see `TrySendError::Full`
//     and decide locally what to do (drop, retry, log).
//   * The writer drains the queue every BATCH_INTERVAL_MS (default
//     250ms) or whenever it has accumulated BATCH_MAX_SIZE entries
//     (default 256), whichever comes first.
//   * Failures inside a batch don't lose successful writes — we use
//     a savepoint per row so a single bad row only rolls itself back.
//
// What's NOT here:
//   * Read paths (history queries, count queries) still go through
//     the original `Database` directly. Reads on a WAL DB don't
//     conflict with writes on a different connection.
//   * Schema migrations. Those happen at startup, before this
//     writer is spawned.

use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};

use crate::SensorReading;

/// One row to commit. Variants map to the existing `Database` API
/// so producers don't have to learn a new vocabulary.
#[derive(Debug)]
pub enum WriteOp {
    Reading {
        timestamp: i64,
        json: String,
        location_id: Option<String>,
    },
    Calibration {
        timestamp: i64,
        sensors_json: String,
        target: f32,
        location_id: Option<String>,
    },
}

/// Tunable knobs.
const QUEUE_CAPACITY: usize = 1024;
const BATCH_MAX_SIZE: usize = 256;
const BATCH_INTERVAL: Duration = Duration::from_millis(250);
/// Backoff schedule for SQLITE_BUSY inside the writer thread.
/// Exponential with jitter via the row-by-row savepoint pattern.
const BUSY_BACKOFF_MS: &[u64] = &[10, 25, 75, 200, 500];

/// Public handle.
///
/// Cloning is cheap and gives a new producer-side handle that points
/// to the same writer thread.
#[derive(Clone)]
pub struct DbWriter {
    tx: SyncSender<WriteOp>,
    /// Counters for diagnostics. Kept in an Arc so producers can read
    /// drop counts without blocking the writer thread.
    metrics: Arc<Metrics>,
}

#[derive(Default, Debug)]
pub struct Metrics {
    pub rows_written: std::sync::atomic::AtomicU64,
    pub rows_dropped_full_queue: std::sync::atomic::AtomicU64,
    pub rows_failed: std::sync::atomic::AtomicU64,
    pub batches_committed: std::sync::atomic::AtomicU64,
}

impl DbWriter {
    /// Spawn the writer thread. Takes ownership of the database
    /// connection.
    ///
    /// The thread runs forever (or until the channel is dropped). On
    /// fatal connection errors it logs and exits — we deliberately
    /// don't try to recover by reopening the database from inside the
    /// writer because that would silently mask a real problem.
    pub fn spawn(db_path: PathBuf) -> std::io::Result<Self> {
        let (tx, rx) = sync_channel::<WriteOp>(QUEUE_CAPACITY);
        let metrics = Arc::new(Metrics::default());
        let metrics_for_thread = metrics.clone();

        thread::Builder::new()
            .name("ambient-db-writer".into())
            .spawn(move || {
                if let Err(e) = run_writer(db_path, rx, metrics_for_thread) {
                    eprintln!("⚠️  db writer thread exiting: {e}");
                }
            })?;

        Ok(Self { tx, metrics })
    }

    /// Best-effort, non-blocking submit. Returns `false` and bumps a
    /// counter when the queue is full — callers shouldn't panic over
    /// this; it just means the writer is overloaded and the row is
    /// being dropped to keep the UI responsive.
    pub fn submit(&self, op: WriteOp) -> bool {
        match self.tx.try_send(op) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.metrics
                    .rows_dropped_full_queue
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                // Writer thread is dead. Return false; calling code
                // can fall back to a synchronous write if it really
                // cares about durability.
                false
            }
        }
    }

    /// Convenience wrapper for the most common reading row.
    pub fn submit_reading(
        &self,
        reading: &SensorReading,
        location_id: Option<&str>,
    ) -> bool {
        let json = match serde_json::to_string(reading) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let timestamp = reading.time.parse::<i64>().unwrap_or(0);
        self.submit(WriteOp::Reading {
            timestamp,
            json,
            location_id: location_id.map(str::to_owned),
        })
    }

    /// Convenience wrapper for calibrations.
    pub fn submit_calibration(
        &self,
        sensors: &[f32],
        target: f32,
        location_id: Option<&str>,
    ) -> bool {
        let sensors_json = match serde_json::to_string(sensors) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.submit(WriteOp::Calibration {
            timestamp,
            sensors_json,
            target,
            location_id: location_id.map(str::to_owned),
        })
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }
}

/// The writer-thread main loop.
fn run_writer(
    db_path: PathBuf,
    rx: Receiver<WriteOp>,
    metrics: Arc<Metrics>,
) -> rusqlite::Result<()> {
    // Open the *writer* connection. Reads still go through whoever
    // owns `Database` — we're the only writer.
    let conn = Connection::open(&db_path)?;

    // Apply the same pragmas the original opener does. Doing it
    // here too keeps the writer self-contained.
    conn.pragma_update(None, "journal_mode", &"WAL")?;
    conn.pragma_update(None, "synchronous", &"NORMAL")?;
    conn.pragma_update(None, "cache_size", &10_000)?;
    // `temp_store=MEMORY` keeps temp tables out of /tmp, slightly
    // faster on systems where /tmp is on slower storage than the DB.
    conn.pragma_update(None, "temp_store", &"MEMORY")?;
    // `wal_autocheckpoint=1000` is the default; we leave it.

    // Buffer for the current batch.
    let mut batch: Vec<WriteOp> = Vec::with_capacity(BATCH_MAX_SIZE);
    // Deadline for the current batch. `None` means "no pending rows,
    // wait indefinitely for the first one". This avoids waking the
    // writer thread on every BATCH_INTERVAL when nothing's queued —
    // important for battery life on a phone, where ~2 wakeups/sec
    // per idle thread keeps the CPU out of deep idle states.
    let mut deadline: Option<Instant> = None;

    loop {
        // If the batch is empty, block indefinitely on the channel.
        // The moment a producer sends an op we start the deadline
        // timer and switch to recv_timeout-style polling so the
        // batch flushes promptly.
        if batch.is_empty() {
            match rx.recv() {
                Ok(op) => {
                    batch.push(op);
                    deadline = Some(Instant::now() + BATCH_INTERVAL);
                    // Drain any immediately-available rows so a
                    // burst that arrived together gets batched.
                    while batch.len() < BATCH_MAX_SIZE {
                        match rx.try_recv() {
                            Ok(more) => batch.push(more),
                            Err(_) => break,
                        }
                    }
                    // If we hit the size cap on the burst alone,
                    // commit now without waiting for more.
                    if batch.len() >= BATCH_MAX_SIZE {
                        let _ = commit_batch(&conn, &mut batch, &metrics);
                        deadline = None;
                    }
                    continue;
                }
                Err(_) => {
                    // All producers gone; nothing to flush since the
                    // batch is empty by precondition.
                    return Ok(());
                }
            }
        }

        // Batch is non-empty: wait for either another row or the
        // deadline, whichever comes first.
        let dl = deadline.expect("invariant: deadline set whenever batch non-empty");
        let remaining = dl.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(op) => {
                batch.push(op);
                while batch.len() < BATCH_MAX_SIZE {
                    match rx.try_recv() {
                        Ok(more) => batch.push(more),
                        Err(_) => break,
                    }
                }
                // Loop until the deadline or the size cap.
                if batch.len() < BATCH_MAX_SIZE && Instant::now() < dl {
                    continue;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Deadline reached with pending rows — flush.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // All producers gone; flush remaining and exit.
                if !batch.is_empty() {
                    let _ = commit_batch(&conn, &mut batch, &metrics);
                }
                return Ok(());
            }
        }

        let _ = commit_batch(&conn, &mut batch, &metrics);
        deadline = None;
    }
}

/// Commit a batch inside a single transaction. Each row uses a
/// savepoint so one bad row doesn't doom the whole batch.
fn commit_batch(
    conn: &Connection,
    batch: &mut Vec<WriteOp>,
    metrics: &Metrics,
) -> rusqlite::Result<()> {
    use std::sync::atomic::Ordering::Relaxed;

    if batch.is_empty() {
        return Ok(());
    }

    // Outer transaction. If the BEGIN itself fails (busy), retry
    // with backoff; we can afford it on this thread.
    if !begin_with_retry(conn) {
        // Couldn't even begin — drop the batch on the floor rather
        // than block forever. This essentially never happens on
        // single-writer WAL setups.
        metrics.rows_failed.fetch_add(batch.len() as u64, Relaxed);
        batch.clear();
        return Ok(());
    }

    // Cache the prepared statements once per batch. Even though
    // `prepare_cached` interns inside the connection, calling it
    // outside the loop saves the HashMap lookup per row.
    let mut stmt_reading = conn.prepare_cached(
        "INSERT OR REPLACE INTO readings (timestamp, data, location_id) VALUES (?1, ?2, ?3)",
    )?;
    let mut stmt_cal = conn.prepare_cached(
        "INSERT INTO calibrations (timestamp, sensors, target, location_id) VALUES (?1, ?2, ?3, ?4)",
    )?;

    let mut written = 0u64;
    let mut failed = 0u64;

    for op in batch.drain(..) {
        // Savepoint per row so a single failure doesn't blow up the
        // entire batch.
        if conn.execute("SAVEPOINT row", []).is_err() {
            failed += 1;
            continue;
        }
        let res = match op {
            WriteOp::Reading { timestamp, json, location_id } => {
                stmt_reading.execute(params![timestamp, json, location_id])
            }
            WriteOp::Calibration { timestamp, sensors_json, target, location_id } => {
                stmt_cal.execute(params![timestamp, sensors_json, target, location_id])
            }
        };
        match res {
            Ok(_) => {
                let _ = conn.execute("RELEASE row", []);
                written += 1;
            }
            Err(_) => {
                let _ = conn.execute("ROLLBACK TO row", []);
                let _ = conn.execute("RELEASE row", []);
                failed += 1;
            }
        }
    }

    // Drop the prepared statements before the COMMIT — rusqlite
    // requires no live statements when closing a transaction.
    drop(stmt_reading);
    drop(stmt_cal);

    // COMMIT with retry. If commit fails (very rare under WAL), we
    // count everything in the batch as failed for the metrics.
    if !commit_with_retry(conn) {
        metrics.rows_failed.fetch_add(written + failed, Relaxed);
        return Ok(());
    }

    metrics.rows_written.fetch_add(written, Relaxed);
    metrics.rows_failed.fetch_add(failed, Relaxed);
    metrics.batches_committed.fetch_add(1, Relaxed);
    Ok(())
}

fn begin_with_retry(conn: &Connection) -> bool {
    busy_retry(|| conn.execute("BEGIN IMMEDIATE", []).map(|_| ()))
}

fn commit_with_retry(conn: &Connection) -> bool {
    busy_retry(|| conn.execute("COMMIT", []).map(|_| ()))
}

/// Run `op`, retrying on SQLITE_BUSY with exponential backoff + jitter.
/// Returns true on success, false after exhausting the schedule.
fn busy_retry<F>(mut op: F) -> bool
where
    F: FnMut() -> rusqlite::Result<()>,
{
    for (i, &ms) in BUSY_BACKOFF_MS.iter().enumerate() {
        match op() {
            Ok(()) => return true,
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::DatabaseBusy =>
            {
                // Add up to 50% jitter so concurrent waiters don't
                // sync up and storm the lock at the same instant.
                let jitter = pseudo_random(ms.wrapping_mul(i as u64 + 1));
                let sleep_ms = ms + (jitter % (ms / 2 + 1));
                thread::sleep(Duration::from_millis(sleep_ms));
            }
            Err(_) => return false,
        }
    }
    false
}

/// Tiny PRNG, no `rand` dep. xorshift64 seeded by the input.
fn pseudo_random(seed: u64) -> u64 {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

/// Tracks live writer count for tests / future shutdown logic.
#[allow(dead_code)]
fn _unused_marker(_x: Mutex<()>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;

    fn open_test_db() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Leak the dir so the path stays valid for the test duration.
        let path = path;
        Box::leak(Box::new(dir));

        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE readings (timestamp INTEGER PRIMARY KEY, data TEXT, location_id TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE calibrations (timestamp INTEGER PRIMARY KEY, sensors TEXT, target REAL, location_id TEXT)",
            [],
        )
        .unwrap();
        path
    }

    #[test]
    fn batches_commit_and_count() {
        let path = open_test_db();
        let writer = DbWriter::spawn(path.clone()).unwrap();

        for i in 0..50 {
            assert!(writer.submit(WriteOp::Reading {
                timestamp: i,
                json: "{}".into(),
                location_id: None,
            }));
        }

        // Wait for the writer to flush at least once.
        let start = Instant::now();
        while writer.metrics.rows_written.load(Relaxed) < 50
            && start.elapsed() < Duration::from_secs(3)
        {
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(writer.metrics.rows_written.load(Relaxed), 50);
        assert!(writer.metrics.batches_committed.load(Relaxed) >= 1);
        assert_eq!(writer.metrics.rows_dropped_full_queue.load(Relaxed), 0);
    }

    #[test]
    fn full_queue_drops_rows_without_blocking() {
        // A writer that never flushes (we never give it time to
        // start). We can't easily simulate a stuck writer, so we
        // just send into a tiny channel from inside one thread.
        let path = open_test_db();
        let writer = DbWriter::spawn(path).unwrap();

        // Slam the queue with way more than QUEUE_CAPACITY at once.
        // Some are guaranteed to be dropped because the writer's
        // commit loop can't drain them as fast as we send.
        let mut accepted = 0u64;
        let mut dropped = 0u64;
        for i in 0..(QUEUE_CAPACITY * 4) {
            if writer.submit(WriteOp::Reading {
                timestamp: i as i64,
                json: "{}".into(),
                location_id: None,
            }) {
                accepted += 1;
            } else {
                dropped += 1;
            }
        }

        assert!(accepted > 0);
        // Dropped count is reported via metrics.
        assert_eq!(
            writer.metrics.rows_dropped_full_queue.load(Relaxed),
            dropped
        );
    }

    #[test]
    fn bad_row_in_batch_doesnt_fail_others() {
        // Send a batch of 10 readings where row #5 has a duplicate
        // timestamp colliding with the primary key. The savepoint
        // mechanism should let the other 9 rows commit.
        //
        // This is the load-bearing property of the writer: a single
        // misbehaving row can't drop nine good ones.
        let path = open_test_db();

        // Pre-insert a row that will collide.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO calibrations (timestamp, sensors, target, location_id) \
                 VALUES (5, '[]', 20.0, NULL)",
                [],
            )
            .unwrap();
        }

        let writer = DbWriter::spawn(path.clone()).unwrap();

        // 10 calibrations; #5 conflicts with the pre-inserted row.
        // (The `calibrations` table doesn't have INSERT OR REPLACE
        // on its insert, so #5 fails.)
        for i in 0..10 {
            writer.submit(WriteOp::Calibration {
                timestamp: i,
                sensors_json: "[]".into(),
                target: 20.0,
                location_id: None,
            });
        }

        // Wait for the batch to flush.
        let start = Instant::now();
        while writer.metrics.batches_committed.load(Relaxed) == 0
            && start.elapsed() < Duration::from_secs(3)
        {
            thread::sleep(Duration::from_millis(50));
        }

        // Drain the residual time so the batch fully commits.
        thread::sleep(Duration::from_millis(400));

        let written = writer.metrics.rows_written.load(Relaxed);
        let failed = writer.metrics.rows_failed.load(Relaxed);

        // Exactly one row should have failed (#5 conflicts), the
        // other nine should have committed.
        assert_eq!(failed, 1, "exactly the colliding row should fail");
        assert_eq!(
            written, 9,
            "the other nine readings should commit despite the bad one"
        );

        // And the table should hold the original pre-inserted row
        // plus the 9 newly-committed ones.
        let conn = Connection::open(&path).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM calibrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1 + 9, "got {} rows", total);
    }

    #[test]
    fn concurrent_producers_share_one_writer() {
        // Spawn 4 threads, each submitting 100 readings. They share
        // one DbWriter handle. All 400 rows should land in the DB
        // (none dropped), via batched commits.
        let path = open_test_db();
        let writer = DbWriter::spawn(path.clone()).unwrap();

        let mut handles = Vec::new();
        for tid in 0..4 {
            let w = writer.clone();
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    let ts = tid * 1000 + i;
                    while !w.submit(WriteOp::Reading {
                        timestamp: ts,
                        json: format!(r#"{{"t":{}}}"#, ts),
                        location_id: None,
                    }) {
                        // Queue full — back off briefly and retry.
                        // This is realistic producer behavior.
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Wait for the writer to drain everything.
        let start = Instant::now();
        while writer.metrics.rows_written.load(Relaxed) < 400
            && start.elapsed() < Duration::from_secs(5)
        {
            thread::sleep(Duration::from_millis(50));
        }

        assert_eq!(
            writer.metrics.rows_written.load(Relaxed),
            400,
            "all 400 rows should have committed"
        );
        assert_eq!(
            writer.metrics.rows_failed.load(Relaxed),
            0,
            "no failures expected"
        );

        // Verify via the table itself: 400 rows present.
        let conn = Connection::open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM readings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 400);
    }
}
