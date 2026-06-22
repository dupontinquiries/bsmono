# Code Patch: XDG Paths Implementation

## Step 1: Update Cargo.toml

Add after line 28 (after `uuid` dependency):

```toml
directories = "5.0"  # XDG Base Directory support
```

---

## Step 2: Add to main.rs imports (after line 46)

```rust
use directories::ProjectDirs;
```

---

## Step 3: Add path helper functions (after constants, before get_latest_database)

Add this entire section around line 2095:

```rust
// ============================================================================
// XDG Base Directory Support
// ============================================================================

/// Get application data directory using XDG specification
/// Falls back to ./weather_data if XDG not available
fn get_app_data_dir() -> PathBuf {
    if let Some(proj_dirs) = ProjectDirs::from("org", "localweather", "ambient-learner") {
        let data_dir = proj_dirs.data_dir().to_path_buf();
        fs::create_dir_all(&data_dir).ok();
        data_dir
    } else {
        // Fallback for systems without XDG
        let fallback = PathBuf::from("weather_data");
        fs::create_dir_all(&fallback).ok();
        fallback
    }
}

/// Get database directory (~/.local/share/ambient-learner/databases)
fn get_database_dir() -> PathBuf {
    let data_dir = get_app_data_dir();
    let db_dir = data_dir.join("databases");
    fs::create_dir_all(&db_dir).ok();
    db_dir
}

/// Get models directory (~/.local/share/ambient-learner/models)
fn get_models_dir() -> PathBuf {
    let data_dir = get_app_data_dir();
    let models_dir = data_dir.join("models");
    fs::create_dir_all(&models_dir).ok();
    models_dir
}

/// Get cache directory (~/.cache/ambient-learner)
fn get_cache_dir() -> PathBuf {
    if let Some(proj_dirs) = ProjectDirs::from("org", "localweather", "ambient-learner") {
        let cache_dir = proj_dirs.cache_dir().to_path_buf();
        fs::create_dir_all(&cache_dir).ok();
        cache_dir
    } else {
        let fallback = PathBuf::from(".cache");
        fs::create_dir_all(&fallback).ok();
        fallback
    }
}

/// Migrate legacy data from ./weather_data to XDG location
fn migrate_legacy_data() {
    let old_dir = PathBuf::from("weather_data");
    
    // Skip if old directory doesn't exist
    if !old_dir.exists() {
        return;
    }
    
    let new_db_dir = get_database_dir();
    let new_models_dir = get_models_dir();
    
    println!("📦 Found legacy data directory, migrating to XDG location...");
    println!("   Old: {}", old_dir.display());
    println!("   New: {}", get_app_data_dir().display());
    
    let mut migrated_count = 0;
    
    // Migrate all files
    if let Ok(entries) = fs::read_dir(&old_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(filename) = path.file_name() {
                    let name = filename.to_string_lossy();
                    
                    // Databases go to databases/
                    if name.ends_with(".db") {
                        let new_path = new_db_dir.join(filename);
                        if !new_path.exists() {
                            if fs::copy(&path, &new_path).is_ok() {
                                println!("   ✓ Migrated database: {}", name);
                                migrated_count += 1;
                            }
                        }
                    }
                    // Models go to models/
                    else if (name.starts_with("model_") || name.starts_with("neural_field_")) 
                             && name.ends_with(".json") {
                        let new_path = new_models_dir.join(filename);
                        if !new_path.exists() {
                            if fs::copy(&path, &new_path).is_ok() {
                                println!("   ✓ Migrated model: {}", name);
                                migrated_count += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    
    if migrated_count > 0 {
        println!("✓ Migration complete! Migrated {} files", migrated_count);
        println!("📁 Data location: {}", get_app_data_dir().display());
        println!("💡 Old ./weather_data directory can be safely deleted");
    } else {
        println!("ℹ️  No files needed migration");
    }
}
```

---

## Step 4: Update get_latest_database() function

**FIND** (around line 2100):
```rust
fn get_latest_database() -> Option<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();
    
    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
```

**REPLACE WITH**:
```rust
fn get_latest_database() -> Option<PathBuf> {
    let db_dir = get_database_dir();
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();
    
    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(db_dir)
```

---

## Step 5: Update list_all_databases() function

**FIND** (around line 2124):
```rust
fn list_all_databases() -> Vec<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();
    
    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
```

**REPLACE WITH**:
```rust
fn list_all_databases() -> Vec<PathBuf> {
    let db_dir = get_database_dir();
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();
    
    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(db_dir)
```

---

## Step 6: Update get_latest_model() function

**FIND** (around line 2150):
```rust
fn get_latest_model() -> Option<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"model_(\d{8}_\d{6})\.json").unwrap();
    
    let mut models: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
```

**REPLACE WITH**:
```rust
fn get_latest_model() -> Option<PathBuf> {
    let models_dir = get_models_dir();
    let re = Regex::new(r"model_(\d{8}_\d{6})\.json").unwrap();
    
    let mut models: Vec<(String, PathBuf)> = fs::read_dir(models_dir)
```

---

## Step 7: Update get_latest_neural_field() function

**FIND** (search for "get_latest_neural_field"):
```rust
fn get_latest_neural_field() -> Option<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"neural_field_(\d{8}_\d{6})\.json").unwrap();
    
    let mut fields: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
```

**REPLACE WITH**:
```rust
fn get_latest_neural_field() -> Option<PathBuf> {
    let models_dir = get_models_dir();
    let re = Regex::new(r"neural_field_(\d{8}_\d{6})\.json").unwrap();
    
    let mut fields: Vec<(String, PathBuf)> = fs::read_dir(models_dir)
```

---

## Step 8: Update Database::new() to use XDG path

**FIND** (in Database implementation):
```rust
pub fn new(sensor_names: &[String]) -> Self {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let db_path = format!("{}/sensors_{}.db", DATA_DIR, timestamp);
```

**REPLACE WITH**:
```rust
pub fn new(sensor_names: &[String]) -> Self {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let db_dir = get_database_dir();
    let db_path = db_dir.join(format!("sensors_{}.db", timestamp));
```

---

## Step 9: Update Model::save() to use XDG path

**FIND** (in Model implementation, search for "model.save"):
```rust
pub fn save(&self) -> std::io::Result<()> {
    fs::create_dir_all(DATA_DIR)?;
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let path = format!("{}/model_{}.json", DATA_DIR, timestamp);
```

**REPLACE WITH**:
```rust
pub fn save(&self) -> std::io::Result<()> {
    let models_dir = get_models_dir();
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let path = models_dir.join(format!("model_{}.json", timestamp));
```

---

## Step 10: Update TemperatureField::save() to use XDG path

**FIND** (in temperature_field.rs save function):
```rust
pub fn save(&self, base_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(base_path)?;
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = format!("{}/neural_field_{}.json", base_path, timestamp);
```

**REPLACE WITH**:
```rust
pub fn save(&self, models_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(models_dir)?;
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = models_dir.join(format!("neural_field_{}.json", timestamp));
```

---

## Step 11: Call migration in main()

**FIND** in run_gtk_ui() (around line 3100):
```rust
fn run_gtk_ui() {
    println!("🎨 Launching GTK4 UI...");
    
    // Use NON_UNIQUE flag to avoid D-Bus registration issues
```

**ADD BEFORE** the application setup:
```rust
fn run_gtk_ui() {
    println!("🎨 Launching GTK4 UI...");
    
    // Migrate legacy data before initialization
    migrate_legacy_data();
    
    // Use NON_UNIQUE flag to avoid D-Bus registration issues
```

**ALSO ADD** to run_iced_ui() and run_headless():
```rust
fn run_iced_ui() {
    println!("🎨 Launching Iced UI...");
    
    // Migrate legacy data
    migrate_legacy_data();
    
    // Initialize core systems
```

---

## Step 12: Update save calls in main.rs

**FIND** calls like:
```rust
neural_field.save(DATA_DIR)?;
```

**REPLACE WITH**:
```rust
let models_dir = get_models_dir();
neural_field.save(&models_dir)?;
```

---

## Verification

After applying all changes, verify:

```bash
# Should find these:
grep "get_database_dir" src/main.rs
grep "get_models_dir" src/main.rs
grep "migrate_legacy_data" src/main.rs
grep "directories::ProjectDirs" src/main.rs

# Should NOT find these anymore:
grep "DATA_DIR" src/main.rs | grep -v "pub const"  # Only constant definition should remain
```

---

## Test

```bash
# Build
cargo build --release

# Run - should show migration
./target/release/ambient-learner

# Expected output:
📦 Found legacy data directory, migrating to XDG location...
   Old: weather_data
   New: /home/user/.local/share/ambient-learner
   ✓ Migrated database: sensors_20250320_143022.db
   ✓ Migrated model: model_20250320_143022.json
✓ Migration complete! Migrated 2 files
📁 Data location: /home/user/.local/share/ambient-learner
💡 Old ./weather_data directory can be safely deleted

# Check new location
ls -la ~/.local/share/ambient-learner/databases/
ls -la ~/.local/share/ambient-learner/models/
```

Done! All data now in standard XDG location.
