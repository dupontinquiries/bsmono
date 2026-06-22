// ============================================================================
// Database Management Module - Functional Programming Approach
// ============================================================================
//
// Pure functional utilities for database maintenance, validation, and recovery
// All functions are stateless, testable, and composable
//

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

// ============================================================================
// Core Types
// ============================================================================

#[derive(Debug, Clone)]
pub struct DbStats {
    pub calibration_count: usize,
    pub reading_count: usize,
    pub oldest_timestamp: Option<i64>,
    pub newest_timestamp: Option<i64>,
    pub data_span_days: i64,
    pub database_size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub integrity_ok: bool,
    pub has_calibrations: bool,
    pub has_readable_data: bool,
    pub corruption_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CalibrationRecord {
    pub timestamp: i64,
    pub sensors: String,
    pub target: f32,
    pub location_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BadSector {
    pub table: String,
    pub row_id: i64,
    pub timestamp: i64,
    pub error: String,
}

// ============================================================================
// Pure Functions - Database Statistics
// ============================================================================

/// Get comprehensive database statistics
pub fn get_stats(conn: &Connection) -> Result<DbStats, rusqlite::Error> {
    let cal_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM calibrations",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    
    let read_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM readings",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    
    let oldest: Option<i64> = conn.query_row(
        "SELECT MIN(timestamp) FROM calibrations",
        [],
        |row| row.get(0)
    ).ok();
    
    let newest: Option<i64> = conn.query_row(
        "SELECT MAX(timestamp) FROM calibrations",
        [],
        |row| row.get(0)
    ).ok();
    
    let data_span_days = match (oldest, newest) {
        (Some(o), Some(n)) => (n - o) / 86400,
        _ => 0,
    };
    
    let database_size_bytes = 0; // Will be populated by caller from file size
    
    Ok(DbStats {
        calibration_count: cal_count as usize,
        reading_count: read_count as usize,
        oldest_timestamp: oldest,
        newest_timestamp: newest,
        data_span_days,
        database_size_bytes,
    })
}

/// Get file size for a database
pub fn get_file_size(path: &Path) -> Result<u64, std::io::Error> {
    let metadata = std::fs::metadata(path)?;
    Ok(metadata.len())
}

/// Calculate database efficiency (data vs file size)
pub fn calculate_efficiency(stats: &DbStats) -> f64 {
    if stats.database_size_bytes == 0 {
        return 1.0;
    }
    
    let expected_size = (stats.calibration_count + stats.reading_count) * 100; // ~100 bytes per record
    expected_size as f64 / stats.database_size_bytes as f64
}

// ============================================================================
// Pure Functions - Database Validation
// ============================================================================

/// Perform comprehensive database validation
pub fn validate_database(conn: &Connection) -> Result<ValidationResult, rusqlite::Error> {
    let mut result = ValidationResult {
        is_valid: true,
        integrity_ok: false,
        has_calibrations: false,
        has_readable_data: false,
        corruption_errors: Vec::new(),
    };
    
    // 1. Check integrity
    match conn.pragma_query_value(None, "integrity_check", |row| row.get::<_, String>(0)) {
        Ok(integrity) => {
            result.integrity_ok = integrity == "ok";
            if !result.integrity_ok {
                result.is_valid = false;
                result.corruption_errors.push(format!("Integrity check failed: {}", integrity));
            }
        }
        Err(e) => {
            result.is_valid = false;
            result.corruption_errors.push(format!("Integrity check error: {}", e));
        }
    }
    
    // 2. Check for calibrations
    let cal_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM calibrations",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    
    result.has_calibrations = cal_count > 0;
    
    // 3. Try to read a calibration
    if result.has_calibrations {
        let readable = conn.query_row(
            "SELECT timestamp, sensors, target FROM calibrations LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, f32>(2)?))
        );
        
        match readable {
            Ok(_) => result.has_readable_data = true,
            Err(e) => {
                result.is_valid = false;
                result.corruption_errors.push(format!("Cannot read calibrations: {}", e));
            }
        }
    }
    
    Ok(result)
}

/// Check if database has minimum viable data
pub fn has_minimum_data(stats: &DbStats) -> bool {
    stats.calibration_count >= 3 // At least 3 calibrations to be useful
}

// ============================================================================
// Pure Functions - Bad Sector Detection
// ============================================================================

/// Scan for corrupted calibration records
pub fn find_bad_calibrations(conn: &Connection) -> Result<Vec<BadSector>, rusqlite::Error> {
    let mut bad_sectors = Vec::new();
    
    let mut stmt = conn.prepare(
        "SELECT rowid, timestamp, sensors, target FROM calibrations"
    )?;
    
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f32>(3)?,
        ))
    })?;
    
    for (idx, row_result) in rows.enumerate() {
        match row_result {
            Ok((rowid, timestamp, sensors, target)) => {
                // Validate data
                let mut errors = Vec::new();
                
                // Check timestamp is reasonable (not in far future or past)
                let now = chrono::Utc::now().timestamp();
                if timestamp < 946684800 || timestamp > now + 86400 {
                    // Before 2000-01-01 or more than 1 day in future
                    errors.push("Invalid timestamp".to_string());
                }
                
                // Check temperature is reasonable (-100 to 150°C)
                if target < -100.0 || target > 150.0 || target.is_nan() || target.is_infinite() {
                    errors.push("Invalid temperature".to_string());
                }
                
                // Check sensors JSON is parseable
                if serde_json::from_str::<Vec<f32>>(&sensors).is_err() {
                    errors.push("Corrupted sensor data".to_string());
                }
                
                if !errors.is_empty() {
                    bad_sectors.push(BadSector {
                        table: "calibrations".to_string(),
                        row_id: rowid,
                        timestamp,
                        error: errors.join(", "),
                    });
                }
            }
            Err(e) => {
                // Row is completely unreadable
                bad_sectors.push(BadSector {
                    table: "calibrations".to_string(),
                    row_id: idx as i64,
                    timestamp: 0,
                    error: format!("Unreadable row: {}", e),
                });
            }
        }
    }
    
    Ok(bad_sectors)
}

/// Scan for corrupted reading records
pub fn find_bad_readings(conn: &Connection) -> Result<Vec<BadSector>, rusqlite::Error> {
    let mut bad_sectors = Vec::new();
    
    let mut stmt = conn.prepare(
        "SELECT rowid, timestamp, data FROM readings"
    )?;
    
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    
    for (idx, row_result) in rows.enumerate() {
        match row_result {
            Ok((rowid, timestamp, data)) => {
                let mut errors = Vec::new();
                
                // Check timestamp
                let now = chrono::Utc::now().timestamp();
                if timestamp < 946684800 || timestamp > now + 86400 {
                    errors.push("Invalid timestamp".to_string());
                }
                
                // Check data is valid JSON
                if serde_json::from_str::<serde_json::Value>(&data).is_err() {
                    errors.push("Corrupted JSON data".to_string());
                }
                
                if !errors.is_empty() {
                    bad_sectors.push(BadSector {
                        table: "readings".to_string(),
                        row_id: rowid,
                        timestamp,
                        error: errors.join(", "),
                    });
                }
            }
            Err(e) => {
                bad_sectors.push(BadSector {
                    table: "readings".to_string(),
                    row_id: idx as i64,
                    timestamp: 0,
                    error: format!("Unreadable row: {}", e),
                });
            }
        }
    }
    
    Ok(bad_sectors)
}

/// Find all bad sectors in database
pub fn scan_for_corruption(conn: &Connection) -> Result<Vec<BadSector>, rusqlite::Error> {
    let mut all_bad = Vec::new();
    
    all_bad.extend(find_bad_calibrations(conn)?);
    all_bad.extend(find_bad_readings(conn)?);
    
    Ok(all_bad)
}

// ============================================================================
// Pure Functions - Data Pruning
// ============================================================================

/// Remove bad sectors from database (returns count of deleted rows)
pub fn prune_bad_sectors(conn: &Connection, bad_sectors: &[BadSector]) -> Result<usize, rusqlite::Error> {
    let mut deleted = 0;
    
    for sector in bad_sectors {
        let count = conn.execute(
            &format!("DELETE FROM {} WHERE rowid = ?1", sector.table),
            params![sector.row_id],
        )?;
        deleted += count;
    }
    
    Ok(deleted)
}

/// Remove old readings (keep only last N days)
pub fn prune_old_readings(conn: &Connection, keep_days: i64) -> Result<usize, rusqlite::Error> {
    let cutoff = chrono::Utc::now().timestamp() - (keep_days * 86400);
    
    let count = conn.execute(
        "DELETE FROM readings WHERE timestamp < ?1",
        params![cutoff],
    )?;
    
    Ok(count)
}

/// Remove duplicate calibrations (keep newest)
pub fn deduplicate_calibrations(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let count = conn.execute(
        "DELETE FROM calibrations WHERE rowid NOT IN (
            SELECT MAX(rowid) FROM calibrations GROUP BY timestamp
        )",
        [],
    )?;
    
    Ok(count)
}

// ============================================================================
// Pure Functions - Database Maintenance
// ============================================================================

/// Vacuum database to reclaim space
pub fn vacuum(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute("VACUUM", [])?;
    Ok(())
}

/// Analyze database to update query planner statistics
pub fn analyze(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute("ANALYZE", [])?;
    Ok(())
}

/// Optimize database (vacuum + analyze)
pub fn optimize(conn: &Connection) -> Result<(), rusqlite::Error> {
    vacuum(conn)?;
    analyze(conn)?;
    Ok(())
}

// ============================================================================
// Pure Functions - Backup and Recovery
// ============================================================================

/// Create a backup of database file
pub fn backup_file(source: &Path, backup_dir: &Path) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(backup_dir)?;
    
    let filename = source.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "No filename")
    })?;
    
    let backup_path = backup_dir.join(format!("{}.backup", filename.to_string_lossy()));
    std::fs::copy(source, &backup_path)?;
    
    Ok(backup_path)
}

/// Restore database from backup
pub fn restore_from_backup(backup: &Path, target: &Path) -> Result<(), std::io::Error> {
    std::fs::copy(backup, target)?;
    Ok(())
}

// ============================================================================
// Composition Functions - High-Level Operations
// ============================================================================

/// Complete database health check (composing multiple checks)
pub fn health_check(conn: &Connection) -> Result<(ValidationResult, Vec<BadSector>), rusqlite::Error> {
    let validation = validate_database(conn)?;
    let bad_sectors = scan_for_corruption(conn)?;
    
    Ok((validation, bad_sectors))
}

/// Full database maintenance (compose all maintenance operations)
pub fn full_maintenance(conn: &Connection, keep_days: i64) -> Result<MaintenanceReport, rusqlite::Error> {
    let stats_before = get_stats(conn)?;
    
    let bad_sectors = scan_for_corruption(conn)?;
    let pruned_bad = prune_bad_sectors(conn, &bad_sectors)?;
    let pruned_old = prune_old_readings(conn, keep_days)?;
    let deduplicated = deduplicate_calibrations(conn)?;
    
    optimize(conn)?;
    
    let stats_after = get_stats(conn)?;
    
    Ok(MaintenanceReport {
        stats_before,
        stats_after,
        bad_sectors_found: bad_sectors.len(),
        bad_sectors_pruned: pruned_bad,
        old_readings_pruned: pruned_old,
        duplicates_removed: deduplicated,
    })
}

#[derive(Debug, Clone)]
pub struct MaintenanceReport {
    pub stats_before: DbStats,
    pub stats_after: DbStats,
    pub bad_sectors_found: usize,
    pub bad_sectors_pruned: usize,
    pub old_readings_pruned: usize,
    pub duplicates_removed: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    
    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        
        conn.execute(
            "CREATE TABLE calibrations (
                timestamp INTEGER PRIMARY KEY,
                sensors TEXT NOT NULL,
                target REAL NOT NULL,
                location_id TEXT
            )",
            [],
        ).unwrap();
        
        conn.execute(
            "CREATE TABLE readings (
                timestamp INTEGER PRIMARY KEY,
                data TEXT NOT NULL,
                location_id TEXT
            )",
            [],
        ).unwrap();
        
        conn
    }
    
    #[test]
    fn test_get_stats_empty() {
        let conn = create_test_db();
        let stats = get_stats(&conn).unwrap();
        
        assert_eq!(stats.calibration_count, 0);
        assert_eq!(stats.reading_count, 0);
        assert_eq!(stats.oldest_timestamp, None);
    }
    
    #[test]
    fn test_get_stats_with_data() {
        let conn = create_test_db();
        
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (1000, '[]', 20.0)",
            [],
        ).unwrap();
        
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (2000, '[]', 21.0)",
            [],
        ).unwrap();
        
        let stats = get_stats(&conn).unwrap();
        
        assert_eq!(stats.calibration_count, 2);
        assert_eq!(stats.oldest_timestamp, Some(1000));
        assert_eq!(stats.newest_timestamp, Some(2000));
    }
    
    #[test]
    fn test_validate_database_healthy() {
        let conn = create_test_db();
        
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000, "[1.0, 2.0]", 20.0],
        ).unwrap();
        
        let result = validate_database(&conn).unwrap();
        
        assert!(result.integrity_ok);
        assert!(result.has_calibrations);
        assert!(result.has_readable_data);
    }
    
    #[test]
    fn test_find_bad_calibrations() {
        let conn = create_test_db();
        
        // Good record
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000000000, "[1.0, 2.0]", 20.0],
        ).unwrap();
        
        // Bad temperature
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000000001, "[1.0, 2.0]", 999.0],
        ).unwrap();
        
        // Bad JSON
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000000002, "not json", 20.0],
        ).unwrap();
        
        let bad = find_bad_calibrations(&conn).unwrap();
        
        assert_eq!(bad.len(), 2); // Two bad records
    }
    
    #[test]
    fn test_prune_bad_sectors() {
        let conn = create_test_db();
        
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000000000, "[1.0]", 20.0],
        ).unwrap();
        
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000000001, "bad", 999.0],
        ).unwrap();
        
        let bad = find_bad_calibrations(&conn).unwrap();
        let deleted = prune_bad_sectors(&conn, &bad).unwrap();
        
        assert_eq!(deleted, 1);
        
        let stats = get_stats(&conn).unwrap();
        assert_eq!(stats.calibration_count, 1); // Only good record remains
    }
    
    #[test]
    fn test_deduplicate() {
        let conn = create_test_db();
        
        // Insert duplicates
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000, "[]", 20.0],
        ).unwrap();
        
        conn.execute(
            "INSERT INTO calibrations (timestamp, sensors, target) VALUES (?, ?, ?)",
            params![1000, "[]", 21.0],
        ).unwrap();
        
        let removed = deduplicate_calibrations(&conn).unwrap();
        assert_eq!(removed, 1);
        
        let stats = get_stats(&conn).unwrap();
        assert_eq!(stats.calibration_count, 1);
    }
}
