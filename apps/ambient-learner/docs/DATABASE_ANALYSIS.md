# Database & History Code Analysis - Issues & Improvements

## 🔍 **Critical Issues Found**

### **1. CRITICAL: Database Backup Race Condition** ⚠️
**Location:** `main.rs:1636-1642`

**Problem:**
```rust
fn open(path: &Path) -> Result<Self, rusqlite::Error> {
    // Create backup before opening (in case of corruption)
    if path.exists() {
        let backup_path = path.with_extension("db.backup");
        if let Err(e) = std::fs::copy(path, &backup_path) {
            eprintln!("⚠️  Warning: Could not create backup: {}", e);
        }
    }
```

**Issues:**
1. Backup is created EVERY time database is opened
2. Overwrites previous backup immediately
3. No timestamp on backups - only one backup exists
4. If corruption happens during copy, backup is also corrupt
5. Database could be modified while being copied (race condition)

**Impact:** 
- Backup protection is essentially useless
- If database corrupts, backup might also be corrupt
- No way to recover to a known-good state

**Recommended Fix:**
```rust
fn create_timestamped_backup(path: &Path) -> Result<PathBuf, std::io::Error> {
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Database does not exist"
        ));
    }
    
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup_path = path.with_file_name(
        format!("{}.backup.{}.db", 
            path.file_stem().unwrap().to_str().unwrap(),
            timestamp
        )
    );
    
    // Use SQLite's backup API instead of file copy
    let source_conn = Connection::open_with_flags(
        path, 
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    )?;
    
    let backup_conn = Connection::open(&backup_path)?;
    
    let backup = rusqlite::backup::Backup::new(
        &source_conn,
        &backup_conn
    )?;
    
    backup.run_to_completion(5, Duration::from_millis(250), None)?;
    
    println!("✓ Created timestamped backup: {}", backup_path.display());
    Ok(backup_path)
}

fn open(path: &Path) -> Result<Self, rusqlite::Error> {
    // Only create backup on first open or after successful operations
    // NOT on every open
    // ...
}
```

---

### **2. CRITICAL: Integer Overflow in History Query** ⚠️
**Location:** `main.rs:1968`

**Problem:**
```rust
pub fn get_calibration_history(&self, days: i64) -> Result<Vec<(i64, f32)>, rusqlite::Error> {
    let cutoff = chrono::Utc::now().timestamp() - (days * 86400);
    
    let query = "SELECT timestamp, target_temp 
                 FROM calibrations 
                 WHERE timestamp > ? 
                 ORDER BY timestamp ASC";
```

**Issues:**
1. Column name is `target_temp` but table has `target`
2. This query will ALWAYS FAIL with "no such column: target_temp"
3. No error handling - silently returns empty vector
4. If `days` is very large (>106751991), overflow possible

**Impact:**
- Calibration history never works
- Silent failure - no error shown to user
- Users think they have no history data

**Recommended Fix:**
```rust
pub fn get_calibration_history(&self, days: i64) -> Result<Vec<(i64, f32)>, rusqlite::Error> {
    // Prevent overflow
    let days = days.min(36500); // Max 100 years
    let cutoff = chrono::Utc::now()
        .timestamp()
        .saturating_sub(days.saturating_mul(86400));
    
    let query = "SELECT timestamp, target   -- FIX: Column is 'target' not 'target_temp'
                 FROM calibrations 
                 WHERE timestamp > ? 
                 ORDER BY timestamp ASC";
    
    let mut stmt = self.conn.prepare(query)?;
    let rows = stmt.query_map([cutoff], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    
    Ok(result)
}
```

---

### **3. HIGH: SQL Injection Vulnerability** 🔴
**Location:** `main.rs:1912`

**Problem:**
```rust
let location_clause = if let Some(loc_id) = location_filter {
    format!("AND location_id = '{}' ", loc_id)  // UNSAFE!
} else {
    String::new()
};

let query = format!(
    "SELECT DATE(timestamp, 'unixepoch') as day, 
            AVG(json_extract(data, '$.sensors[0]')) as avg_temp
     FROM readings 
     WHERE timestamp BETWEEN 
        strftime('%s', 'now', '-{} days') AND 
        strftime('%s', 'now', '-{} days')
        {}
     GROUP BY day
     ORDER BY day DESC",
    days_back + num_days,
    days_back,
    location_clause  // Injected without sanitization!
);
```

**Issues:**
1. Location ID is inserted directly into SQL string
2. No parameterization
3. Attacker could craft location_id to manipulate query

**Impact:**
- Potential SQL injection if location_id comes from user input
- Could leak data, corrupt database, or DOS

**Recommended Fix:**
```rust
fn get_daily_averages(&self, days_back: usize, num_days: usize, location_filter: Option<&str>) 
    -> Result<Vec<(String, f32)>, rusqlite::Error> {
    
    let query = if location_filter.is_some() {
        "SELECT DATE(timestamp, 'unixepoch') as day, 
                AVG(json_extract(data, '$.sensors[0]')) as avg_temp
         FROM readings 
         WHERE timestamp BETWEEN 
            strftime('%s', 'now', '-' || ? || ' days') AND 
            strftime('%s', 'now', '-' || ? || ' days')
            AND location_id = ?
         GROUP BY day
         ORDER BY day DESC"
    } else {
        "SELECT DATE(timestamp, 'unixepoch') as day, 
                AVG(json_extract(data, '$.sensors[0]')) as avg_temp
         FROM readings 
         WHERE timestamp BETWEEN 
            strftime('%s', 'now', '-' || ? || ' days') AND 
            strftime('%s', 'now', '-' || ? || ' days')
         GROUP BY day
         ORDER BY day DESC"
    };
    
    let mut stmt = self.conn.prepare(query)?;
    
    let rows = if let Some(loc) = location_filter {
        stmt.query_map(params![days_back + num_days, days_back, loc], 
            |row| Ok((row.get(0)?, row.get(1)?)))?
    } else {
        stmt.query_map(params![days_back + num_days, days_back],
            |row| Ok((row.get(0)?, row.get(1)?)))?
    };
    
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    
    Ok(result)
}
```

---

### **4. MEDIUM: Unbounded Retry Loop** ⚠️
**Location:** `main.rs:1807-1825`

**Problem:**
```rust
// Retry logic for database locked errors
for attempt in 0..3 {
    match self.conn.execute(
        "INSERT OR REPLACE INTO readings (timestamp, data, location_id) VALUES (?1, ?2, ?3)",
        params![timestamp, json, location_id],
    ) {
        Ok(_) => return Ok(()),
        Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::DatabaseBusy => {
            if attempt < 2 {
                eprintln!("Database busy, retrying... (attempt {})", attempt + 1);
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            return Err(rusqlite::Error::SqliteFailure(err, None));
        }
        Err(e) => return Err(e),
    }
}
```

**Issues:**
1. Fixed 50ms sleep - no exponential backoff
2. Only 3 attempts might not be enough under high contention
3. Sleeps on UI thread (blocks interface)
4. Same pattern duplicated in multiple places (DRY violation)

**Impact:**
- UI freezes during retries
- May still fail under load
- Code duplication

**Recommended Fix:**
```rust
/// Generic retry helper with exponential backoff
fn retry_db_operation<F, T>(operation: F, max_attempts: usize) -> Result<T, rusqlite::Error>
where
    F: Fn() -> Result<T, rusqlite::Error>,
{
    let mut attempt = 0;
    loop {
        match operation() {
            Ok(result) => return Ok(result),
            Err(rusqlite::Error::SqliteFailure(err, _)) 
                if err.code == rusqlite::ErrorCode::DatabaseBusy => {
                
                attempt += 1;
                if attempt >= max_attempts {
                    return Err(rusqlite::Error::SqliteFailure(err, None));
                }
                
                // Exponential backoff: 10ms, 20ms, 40ms, 80ms, ...
                let sleep_ms = 10 * (1 << attempt);
                let sleep_duration = std::time::Duration::from_millis(sleep_ms);
                
                eprintln!("Database busy, retrying in {:?}... (attempt {}/{})", 
                    sleep_duration, attempt, max_attempts);
                
                std::thread::sleep(sleep_duration);
            }
            Err(e) => return Err(e),
        }
    }
}

fn store_reading(&self, reading: &SensorReading, location_id: Option<&str>) 
    -> Result<(), rusqlite::Error> {
    
    let json = serde_json::to_string(reading).unwrap();
    let timestamp: i64 = reading.time.parse().unwrap_or(0);
    
    retry_db_operation(|| {
        self.conn.execute(
            "INSERT OR REPLACE INTO readings (timestamp, data, location_id) VALUES (?1, ?2, ?3)",
            params![timestamp, json, location_id],
        )?;
        Ok(())
    }, 5)
}
```

---

### **5. MEDIUM: Unsafe Timestamp Parsing** ⚠️
**Location:** `main.rs:1804`

**Problem:**
```rust
let timestamp: i64 = reading.time.parse().unwrap_or(0);
```

**Issues:**
1. `unwrap_or(0)` makes ALL failed parses become timestamp 0 (1970-01-01)
2. Silently corrupts data
3. No logging of parse failures
4. Records with timestamp 0 will cause issues in queries

**Impact:**
- Corrupt data silently inserted
- All failed reads map to same timestamp
- Queries ordering by timestamp will be wrong

**Recommended Fix:**
```rust
fn store_reading(&self, reading: &SensorReading, location_id: Option<&str>) 
    -> Result<(), rusqlite::Error> {
    
    let json = serde_json::to_string(reading)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    
    let timestamp: i64 = reading.time.parse()
        .map_err(|e| {
            eprintln!("⚠️  Failed to parse timestamp '{}': {}", reading.time, e);
            rusqlite::Error::InvalidQuery
        })?;
    
    // Validate timestamp is reasonable
    if timestamp < 946684800 || timestamp > chrono::Utc::now().timestamp() + 86400 {
        eprintln!("⚠️  Invalid timestamp: {}", timestamp);
        return Err(rusqlite::Error::InvalidQuery);
    }
    
    retry_db_operation(|| {
        self.conn.execute(
            "INSERT OR REPLACE INTO readings (timestamp, data, location_id) VALUES (?1, ?2, ?3)",
            params![timestamp, json, location_id],
        )?;
        Ok(())
    }, 5)
}
```

---

## 🐛 **Medium-Priority Issues**

### **6. Missing Index on Calibrations Timestamp**
**Location:** `main.rs:1627`

**Problem:**
```rust
conn.execute("CREATE INDEX IF NOT EXISTS idx_readings_time ON readings(timestamp)", [])?;
conn.execute("CREATE INDEX IF NOT EXISTS idx_readings_location ON readings(location_id)", [])?;
// Missing: Index on calibrations(timestamp)!
```

**Impact:**
- Slow queries on calibration history
- `get_calibration_history()` does full table scan
- Performance degrades with more calibrations

**Fix:**
```rust
conn.execute("CREATE INDEX IF NOT EXISTS idx_calibrations_time ON calibrations(timestamp)", [])?;
conn.execute("CREATE INDEX IF NOT EXISTS idx_calibrations_location ON calibrations(location_id)", [])?;
```

---

### **7. No Transaction Wrapping for Batch Operations**
**Location:** `main.rs:1856-1876`

**Problem:**
Bootstrap training loads all calibrations without transaction:
```rust
fn get_all_calibrations(&self) -> Result<Vec<(Vec<f32>, f32)>, rusqlite::Error> {
    let mut stmt = self.conn.prepare(
        "SELECT sensors, target FROM calibrations ORDER BY timestamp"
    )?;
    // ... maps all rows ...
}
```

**Impact:**
- If database is being written to during read, may get inconsistent state
- No ACID guarantees

**Fix:**
```rust
fn get_all_calibrations(&self) -> Result<Vec<(Vec<f32>, f32)>, rusqlite::Error> {
    // Wrap in read transaction
    let tx = self.conn.transaction()?;
    
    let mut stmt = tx.prepare(
        "SELECT sensors, target FROM calibrations ORDER BY timestamp"
    )?;
    
    let calibrations: Vec<(Vec<f32>, f32)> = stmt.query_map([], |row| {
        let sensors_json: String = row.get(0)?;
        let target: f32 = row.get(1)?;
        let sensors: Vec<f32> = serde_json::from_str(&sensors_json)
            .unwrap_or_default();
        Ok((sensors, target))
    })?
    .filter_map(Result::ok)
    .filter(|(sensors, _)| !sensors.is_empty())
    .collect();
    
    tx.commit()?;
    Ok(calibrations)
}
```

---

### **8. Duplicate Code: Database Stats** 
**Locations:** `main.rs:1717`, `dbmaint.rs:56`

**Problem:**
Same logic for getting database stats exists in two places:
- `Database::get_database_stats()`
- `dbmaint::get_stats()`

**Impact:**
- Maintenance burden
- Risk of divergence
- Confusing which to use

**Fix:**
Remove duplication, use dbmaint version everywhere.

---

### **9. No Connection Pooling**

**Problem:**
Every database open creates new connection. With WAL mode, concurrent readers are allowed but only one writer.

**Impact:**
- Contention on writes
- Frequent "database is locked" errors

**Recommended:**
Consider connection pooling with r2d2 or similar.

---

## 🔧 **Code Quality Issues**

### **10. Inconsistent Error Handling**

**Problem:**
Mix of:
- `unwrap_or(0)` (silent failures)
- `unwrap_or_default()` (silent failures)
- `.ok()` (discards errors)
- Proper `Result` returns

**Example:**
```rust
let count: i64 = conn.query_row(...).unwrap_or(0);
```

**Impact:**
- Hard to debug
- Silent data corruption
- Inconsistent behavior

**Fix:**
Standardize on proper error propagation with `?` operator.

---

### **11. Magic Numbers**

**Problem:**
```rust
if timestamp < 946684800 // What is this?
if target < -100.0 || target > 150.0 // Why these values?
keep_days: i64 // How many is reasonable?
```

**Fix:**
```rust
const EPOCH_2000: i64 = 946684800;
const MIN_TEMP_CELSIUS: f32 = -100.0;
const MAX_TEMP_CELSIUS: f32 = 150.0;
const DEFAULT_RETENTION_DAYS: i64 = 90;
const MAX_RETENTION_DAYS: i64 = 36500; // 100 years
```

---

### **12. No Prepared Statement Caching**

**Problem:**
Queries are prepared every time they're called:
```rust
fn get_reading_count(&self) -> Result<usize, rusqlite::Error> {
    let count: i64 = self.conn.query_row(
        "SELECT COUNT(*) FROM readings",
        [],
        |row| row.get(0)
    )?;
    Ok(count as usize)
}
```

**Impact:**
- Performance overhead
- SQLite has to parse SQL every time

**Fix:**
Consider caching prepared statements in Database struct.

---

## 📊 **History UI Issues**

### **13. History Chart Never Updates**
**Location:** `main.rs:3895-3973`

**Problem:**
```rust
let history_picture = gtk::Picture::new();
let history_spinner = gtk::Spinner::new();
// ... but no code to actually load chart data!
```

**Impact:**
- History page shows nothing
- Spinner might spin forever
- No error messages

**Needs:**
- Actually call `get_daily_averages()`
- Generate chart
- Load into Picture widget
- Stop spinner when done

---

### **14. Week Navigation Broken**

**Problem:**
Buttons exist but have no click handlers that update the chart:
```rust
let prev_week_btn = gtk::Button::with_label("◀");
let next_week_btn = gtk::Button::with_label("▶");
// No .connect_clicked() callbacks!
```

---

## 🎯 **Priority Recommendations**

### **Immediate (Critical):**
1. ✅ Fix `get_calibration_history()` column name bug
2. ✅ Fix SQL injection in `get_daily_averages()`
3. ✅ Add proper backup strategy with timestamps
4. ✅ Add timestamp validation in `store_reading()`

### **High Priority:**
5. ✅ Add missing indexes on calibrations table
6. ✅ Implement retry helper with exponential backoff
7. ✅ Add transaction wrapping for batch reads
8. ✅ Actually implement history chart loading

### **Medium Priority:**
9. ⬜ Add prepared statement caching
10. ⬜ Standardize error handling
11. ⬜ Add constants for magic numbers
12. ⬜ Remove duplicate stats code

### **Low Priority:**
13. ⬜ Consider connection pooling
14. ⬜ Add more comprehensive tests
15. ⬜ Add telemetry/metrics

---

## 📈 **Suggested Improvements**

### **A. Add Database Migrations System**
```rust
const DB_VERSION: i32 = 2;

fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Create version table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY)",
        []
    )?;
    
    let current_version: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    
    if current_version < 1 {
        // Migration 1: Add location_id columns
        conn.execute("ALTER TABLE readings ADD COLUMN location_id TEXT", [])?;
        conn.execute("ALTER TABLE calibrations ADD COLUMN location_id TEXT", [])?;
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])?;
    }
    
    if current_version < 2 {
        // Migration 2: Add indexes
        conn.execute("CREATE INDEX idx_calibrations_time ON calibrations(timestamp)", [])?;
        conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])?;
    }
    
    Ok(())
}
```

### **B. Add Query Result Caching**
```rust
use std::collections::HashMap;
use std::time::{Instant, Duration};

struct QueryCache {
    cache: HashMap<String, (Instant, Vec<(String, f32)>)>,
    ttl: Duration,
}

impl QueryCache {
    fn get(&mut self, key: &str) -> Option<Vec<(String, f32)>> {
        if let Some((timestamp, data)) = self.cache.get(key) {
            if timestamp.elapsed() < self.ttl {
                return Some(data.clone());
            }
        }
        None
    }
    
    fn set(&mut self, key: String, data: Vec<(String, f32)>) {
        self.cache.insert(key, (Instant::now(), data));
    }
}
```

### **C. Add Comprehensive Logging**
```rust
use log::{info, warn, error};

fn store_reading(&self, reading: &SensorReading, location_id: Option<&str>) 
    -> Result<(), rusqlite::Error> {
    
    info!("Storing reading: timestamp={}, location={:?}", 
        reading.time, location_id);
    
    // ... operation ...
    
    if let Err(e) = result {
        error!("Failed to store reading: {}", e);
        return Err(e);
    }
    
    info!("Reading stored successfully");
    Ok(())
}
```

---

## ✅ **Summary**

**Critical Issues:** 5  
**High Priority:** 4  
**Medium Priority:** 9  
**Code Quality:** 5  

**Most Urgent Fixes:**
1. Fix broken calibration history query (1 line change!)
2. Fix SQL injection vulnerability
3. Implement proper backup system
4. Add timestamp validation

**Estimated fix time:** 4-8 hours for critical issues

---

## 🧪 **Recommended Tests**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_get_calibration_history() {
        let db = Database::new("test").unwrap();
        
        // Insert test calibration
        db.store_calibration(&vec![25.0; 53], 21.5, None).unwrap();
        
        // Should be able to retrieve it
        let history = db.get_calibration_history(7).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, 21.5);
    }
    
    #[test]
    fn test_sql_injection_protection() {
        let db = Database::new("test").unwrap();
        
        // Try injection
        let malicious = "'; DROP TABLE calibrations; --";
        let result = db.get_daily_averages(0, 7, Some(malicious));
        
        // Should not crash or corrupt database
        assert!(result.is_ok());
        
        // Calibrations table should still exist
        assert!(db.get_calibration_count().is_ok());
    }
}
```
