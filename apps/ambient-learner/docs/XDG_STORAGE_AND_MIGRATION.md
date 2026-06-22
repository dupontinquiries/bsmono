# Database Storage & Legacy Migration Solution

## 🐛 **Current Problems**

### 1. Working Directory-Dependent Storage
**Problem:** Data stored in `./weather_data` relative to where app runs
```
~/Code/app1/weather_data/  ← Instance 1
~/Code/app2/weather_data/  ← Instance 2
~/Downloads/weather_data/   ← If run from Downloads!
```

**Result:** Each location has separate databases, can't share calibrations

### 2. Legacy Database Detection Bug
**Problem:** All databases flagged as "legacy" even when they have correct tables
**Cause:** Logic checks for old table names but doesn't properly validate new format

### 3. No Migration Path
**Problem:** Can't import old databases into neural field training
**Need:** Automatic migration from legacy format to neural-compatible

---

## ✅ **Solution: XDG Base Directory Specification**

### Use Standard Linux Paths:

```
~/.local/share/ambient-learner/databases/    ← All databases here
~/.local/share/ambient-learner/models/       ← Saved models
~/.config/ambient-learner/config.toml        ← Configuration
~/.cache/ambient-learner/sphere/             ← Cached SVGs
```

**Benefits:**
- ✅ Works regardless of where app runs
- ✅ Standard Linux location
- ✅ Survives app reinstalls
- ✅ Easy to backup (`~/.local/share/ambient-learner`)
- ✅ Follows XDG spec
- ✅ No conflicts between instances

---

## 🔧 **Implementation**

### Step 1: Add directories crate to Cargo.toml

```toml
[dependencies]
# Add this:
directories = "5.0"
```

### Step 2: Create paths module in main.rs

```rust
// Add after imports, before constants:

/// Get application data directory using XDG specification
fn get_app_data_dir() -> PathBuf {
    if let Some(proj_dirs) = directories::ProjectDirs::from("org", "localweather", "ambient-learner") {
        proj_dirs.data_dir().to_path_buf()
    } else {
        // Fallback to current directory if XDG not available
        PathBuf::from("weather_data")
    }
}

/// Get database directory
fn get_database_dir() -> PathBuf {
    let data_dir = get_app_data_dir();
    let db_dir = data_dir.join("databases");
    fs::create_dir_all(&db_dir).ok();
    db_dir
}

/// Get models directory
fn get_models_dir() -> PathBuf {
    let data_dir = get_app_data_dir();
    let models_dir = data_dir.join("models");
    fs::create_dir_all(&models_dir).ok();
    models_dir
}

/// Get cache directory
fn get_cache_dir() -> PathBuf {
    if let Some(proj_dirs) = directories::ProjectDirs::from("org", "localweather", "ambient-learner") {
        let cache_dir = proj_dirs.cache_dir().to_path_buf();
        fs::create_dir_all(&cache_dir).ok();
        cache_dir
    } else {
        PathBuf::from(".cache")
    }
}
```

### Step 3: Update DATA_DIR constant

```rust
// OLD:
pub const DATA_DIR: &str = "weather_data";

// NEW: Make it lazy static or use function
// Remove constant, use function everywhere:
// Replace DATA_DIR with get_database_dir()
```

### Step 4: Update database functions

```rust
fn get_latest_database() -> Option<PathBuf> {
    let db_dir = get_database_dir();  // ← Changed
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();
    
    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(db_dir)  // ← Changed
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let filename = path.file_name()?.to_str()?;
            
            if let Some(caps) = re.captures(filename) {
                let timestamp = caps.get(1)?.as_str().to_string();
                Some((timestamp, path))
            } else {
                None
            }
        })
        .collect();
    
    dbs.sort_by(|a, b| b.0.cmp(&a.0));
    dbs.first().map(|(_, path)| path.clone())
}

fn get_latest_model() -> Option<PathBuf> {
    let models_dir = get_models_dir();  // ← New
    let re = Regex::new(r"model_(\d{8}_\d{6})\.json").unwrap();
    
    let mut models: Vec<(String, PathBuf)> = fs::read_dir(models_dir)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let filename = path.file_name()?.to_str()?;
            
            if let Some(caps) = re.captures(filename) {
                let timestamp = caps.get(1)?.as_str().to_string();
                Some((timestamp, path))
            } else {
                None
            }
        })
        .collect();
    
    models.sort_by(|a, b| b.0.cmp(&a.0));
    models.first().map(|(_, path)| path.clone())
}
```

---

## 🔄 **Migration Strategy**

### Auto-migrate on startup:

```rust
/// Migrate old databases from working directory to XDG location
fn migrate_legacy_data() {
    let old_dir = PathBuf::from("weather_data");
    let new_dir = get_database_dir();
    
    // Skip if old directory doesn't exist
    if !old_dir.exists() {
        return;
    }
    
    println!("📦 Found legacy data directory, migrating...");
    
    // Migrate databases
    if let Ok(entries) = fs::read_dir(&old_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "db" {
                    if let Some(filename) = path.file_name() {
                        let new_path = new_dir.join(filename);
                        if !new_path.exists() {
                            if let Err(e) = fs::copy(&path, &new_path) {
                                eprintln!("⚠️  Failed to migrate {}: {}", filename.to_string_lossy(), e);
                            } else {
                                println!("✓ Migrated database: {}", filename.to_string_lossy());
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Migrate models
    let models_dir = get_models_dir();
    if let Ok(entries) = fs::read_dir(&old_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name() {
                let name = filename.to_string_lossy();
                if name.starts_with("model_") && name.ends_with(".json") {
                    let new_path = models_dir.join(filename);
                    if !new_path.exists() {
                        if let Err(e) = fs::copy(&path, &new_path) {
                            eprintln!("⚠️  Failed to migrate {}: {}", name, e);
                        } else {
                            println!("✓ Migrated model: {}", name);
                        }
                    }
                }
                if name.starts_with("neural_field_") && name.ends_with(".json") {
                    let new_path = models_dir.join(filename);
                    if !new_path.exists() {
                        if let Err(e) = fs::copy(&path, &new_path) {
                            eprintln!("⚠️  Failed to migrate {}: {}", name, e);
                        } else {
                            println!("✓ Migrated neural field: {}", name);
                        }
                    }
                }
            }
        }
    }
    
    println!("✓ Migration complete!");
    println!("📁 Data now in: {}", new_dir.display());
    println!("💡 Old data in ./weather_data can be safely deleted");
}

// Call in main() before loading databases:
fn main() {
    // ... CLI parsing ...
    
    // Migrate legacy data first
    migrate_legacy_data();
    
    // ... rest of initialization ...
}
```

---

## 🔧 **Fix Legacy Database Detection**

### Problem: False positives for "legacy" databases

```rust
// Current logic (BROKEN):
// Checks if table exists, but always shows "legacy" warning

// FIXED VERSION:
fn validate_database_schema(db_path: &Path) -> Result<DatabaseVersion, String> {
    let conn = Connection::open(db_path)
        .map_err(|e| format!("Cannot open: {}", e))?;
    
    // Check for modern schema (v2)
    let has_sensor_readings: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sensor_readings'",
        [],
        |row| row.get(0)
    ).unwrap_or(false);
    
    let has_calibrations_v2: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='model_calibrations'",
        [],
        |row| row.get(0)
    ).unwrap_or(false);
    
    let has_neural_training: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='neural_training_data'",
        [],
        |row| row.get(0)
    ).unwrap_or(false);
    
    // Modern schema
    if has_sensor_readings && has_calibrations_v2 {
        if has_neural_training {
            Ok(DatabaseVersion::V2Neural)  // Has neural training data
        } else {
            Ok(DatabaseVersion::V2)  // Modern but no neural yet
        }
    }
    // Legacy schema
    else if has_legacy_tables(&conn) {
        Ok(DatabaseVersion::V1Legacy)
    }
    else {
        Err("Unknown database format".to_string())
    }
}

fn has_legacy_tables(conn: &Connection) -> bool {
    let tables = vec!["calibrations", "data", "sensor_data"];
    for table in tables {
        let exists: bool = conn.query_row(
            &format!("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='{}'", table),
            [],
            |row| row.get(0)
        ).unwrap_or(false);
        
        if exists {
            return true;
        }
    }
    false
}

enum DatabaseVersion {
    V1Legacy,      // Old format, needs migration
    V2,            // Modern format, no neural data yet
    V2Neural,      // Modern format with neural training data
}
```

---

## 📊 **Bootstrap Training from Legacy Databases**

```rust
/// Extract calibrations from any database version
fn extract_calibrations_for_training(db_path: &Path) -> Result<Vec<CalibrationData>, String> {
    let conn = Connection::open(db_path)
        .map_err(|e| format!("Cannot open: {}", e))?;
    
    let version = validate_database_schema(db_path)?;
    
    match version {
        DatabaseVersion::V2Neural | DatabaseVersion::V2 => {
            // Modern schema
            let mut stmt = conn.prepare(
                "SELECT timestamp, sensor_readings, ambient_temp, location 
                 FROM model_calibrations 
                 ORDER BY timestamp DESC"
            ).map_err(|e| format!("Query failed: {}", e))?;
            
            let calibrations = stmt.query_map([], |row| {
                Ok(CalibrationData {
                    timestamp: row.get(0)?,
                    sensors: serde_json::from_str(row.get::<_, String>(1)?.as_str()).unwrap_or_default(),
                    ambient_temp: row.get(2)?,
                    location: row.get::<_, Option<String>>(3)?,
                })
            }).map_err(|e| format!("Map failed: {}", e))?;
            
            calibrations.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Collect failed: {}", e))
        }
        DatabaseVersion::V1Legacy => {
            // Try different legacy table formats
            extract_from_legacy_tables(&conn)
        }
    }
}

fn extract_from_legacy_tables(conn: &Connection) -> Result<Vec<CalibrationData>, String> {
    // Try different legacy table structures
    let queries = vec![
        "SELECT timestamp, sensors, temperature FROM calibrations",
        "SELECT time, sensor_data, ambient FROM data",
        "SELECT ts, readings, temp FROM sensor_data",
    ];
    
    for query in queries {
        if let Ok(mut stmt) = conn.prepare(query) {
            if let Ok(calibrations) = stmt.query_map([], |row| {
                Ok(CalibrationData {
                    timestamp: row.get(0).unwrap_or(0),
                    sensors: serde_json::from_str(
                        row.get::<_, String>(1).unwrap_or_default().as_str()
                    ).unwrap_or_default(),
                    ambient_temp: row.get(2).unwrap_or(20.0),
                    location: None,
                })
            }) {
                let data: Vec<_> = calibrations.flatten().collect();
                if !data.is_empty() {
                    println!("✓ Extracted {} calibrations from legacy database", data.len());
                    return Ok(data);
                }
            }
        }
    }
    
    Err("No compatible data found in legacy database".to_string())
}

struct CalibrationData {
    timestamp: i64,
    sensors: Vec<f32>,
    ambient_temp: f32,
    location: Option<String>,
}
```

---

## 📋 **Complete Implementation Checklist**

### Phase 1: XDG Paths (30 minutes)
- [ ] Add `directories = "5.0"` to Cargo.toml
- [ ] Add path helper functions (get_database_dir, etc.)
- [ ] Replace all `DATA_DIR` with `get_database_dir()`
- [ ] Update model save/load to use `get_models_dir()`
- [ ] Add migration function
- [ ] Call migration in main()

### Phase 2: Schema Validation (20 minutes)
- [ ] Add `DatabaseVersion` enum
- [ ] Add `validate_database_schema()` function
- [ ] Update database import to use validation
- [ ] Show appropriate messages per version

### Phase 3: Legacy Migration (30 minutes)
- [ ] Add `extract_calibrations_for_training()` function
- [ ] Add `extract_from_legacy_tables()` helper
- [ ] Wire up bootstrap training to use extractor
- [ ] Test with old databases

---

## 🚀 **User Experience**

### First Run (no data):
```
🎨 Launching GTK4 UI...
[1/10] Initializing sensors...
✓ Found 53 thermal sensors
...
📁 No existing data found
📁 Data will be saved to: ~/.local/share/ambient-learner/
```

### First Run (with old data):
```
📦 Found legacy data directory, migrating...
✓ Migrated database: sensors_20250320_143022.db
✓ Migrated database: sensors_20250321_091533.db
✓ Migrated model: model_20250321_091533.json
✓ Migrated neural field: neural_field_20250321_091533.json
✓ Migration complete!
📁 Data now in: /home/user/.local/share/ambient-learner/databases
💡 Old data in ./weather_data can be safely deleted

[1/10] Initializing sensors...
```

### Importing Legacy Database:
```
(User clicks "Import Database" and selects old .db file)

Checking database format...
✓ Legacy database format detected (v1)
📊 Found 47 calibrations
🧠 Converting to neural training format...
✓ Imported 47 calibrations for neural field training
🎯 Ready for bootstrap training!
```

---

## 🎯 **Benefits**

**XDG Paths:**
- ✅ Run app from anywhere, same data
- ✅ Standard Linux location
- ✅ Easy backup: `~/.local/share/ambient-learner`
- ✅ Survives reinstalls
- ✅ No duplicate databases

**Schema Validation:**
- ✅ Correctly identifies modern databases
- ✅ Detects legacy format
- ✅ Shows appropriate messages
- ✅ No false "legacy" warnings

**Migration:**
- ✅ Automatic on first run
- ✅ Preserves all data
- ✅ Can import any old database
- ✅ Enables neural field training from old data

---

## 📝 **Example Paths**

```bash
# Before (problem):
~/Code/project1/weather_data/sensors_*.db
~/Downloads/weather_data/sensors_*.db

# After (solution):
~/.local/share/ambient-learner/databases/sensors_*.db
~/.local/share/ambient-learner/models/model_*.json
~/.local/share/ambient-learner/models/neural_field_*.json
~/.config/ambient-learner/config.toml
~/.cache/ambient-learner/sphere/sphere_cache_*.svg
```

**Bonus:** Add to documentation:
```bash
# Backup all data:
tar -czf ambient-learner-backup.tar.gz ~/.local/share/ambient-learner

# Restore:
tar -xzf ambient-learner-backup.tar.gz -C ~
```

---

## 🎉 **Summary**

**Three fixes in one:**
1. ✅ XDG-compliant paths (no more location-dependent storage)
2. ✅ Proper database version detection (no false "legacy" warnings)
3. ✅ Legacy migration (import old databases for neural training)

**Result:** Professional, standards-compliant storage that works anywhere!
