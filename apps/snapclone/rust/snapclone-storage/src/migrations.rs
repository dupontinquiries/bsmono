//! Database migrations — Rust port of StorageManager::applyMigrations().
//!
//! Each migration is identified by a version number. Migrations are applied
//! in order; once applied they are not re-applied (version tracked in
//! PRAGMA user_version).
//!
//! IMPORTANT: PRAGMA user_version must be set OUTSIDE any transaction.
//! Inside a transaction, SQLite may silently discard the pragma on some builds.
//! This is the same bug that caused the Qt C++ version to re-run migrations
//! on every launch, losing contact data.

use anyhow::Result;
use rusqlite::Connection;
use tracing::{info, warn};

/// All migrations in version order. Each tuple is (version, sql).
/// SQL may contain multiple semicolon-separated statements.
static MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("sql/v01_initial.sql")),
    (2, include_str!("sql/v02_snap_indexes.sql")),
    (3, include_str!("sql/v03_snap_recipients.sql")),
    (4, include_str!("sql/v04_peer_addresses.sql")),
    (5, include_str!("sql/v05_user_extended.sql")),
    (6, include_str!("sql/v06_dht_cache.sql")),
    (7, include_str!("sql/v07_peer_locations.sql")),
    (8, include_str!("sql/v08_reset_online.sql")),
    (9, include_str!("sql/v09_contact_state.sql")),
    (10, include_str!("sql/v10_peer_keys.sql")),
    (11, include_str!("sql/v11_video_columns.sql")),
];

pub fn apply(conn: &Connection) -> Result<()> {
    let current: u32 = conn.query_row(
        "PRAGMA user_version", [], |r| r.get(0)
    )?;

    for &(version, sql) in MIGRATIONS {
        if version <= current { continue; }
        info!("Applying migration to version {}", version);
        apply_one(conn, version, sql)?;
    }
    Ok(())
}

fn apply_one(conn: &Connection, version: u32, sql: &str) -> Result<()> {
    // Strip single-line comments (SQLite doesn't always handle them in
    // execute_batch on some builds)
    let stripped: String = sql.lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");

    // Run DDL inside a transaction so partial migrations are rolled back
    conn.execute_batch("BEGIN;")?;
    if let Err(e) = conn.execute_batch(&stripped) {
        let _ = conn.execute_batch("ROLLBACK;");
        warn!("Migration {} failed: {}", version, e);
        return Err(e.into());
    }
    conn.execute_batch("COMMIT;")?;

    // Set user_version OUTSIDE the transaction — inside it may not persist
    conn.execute_batch(&format!("PRAGMA user_version = {};", version))?;
    info!("Migration {} applied", version);
    Ok(())
}
