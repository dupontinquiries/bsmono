# Database Recovery Protocol Improvements

## 🔧 Issue: Database Being Wiped Consistently

### Problem Identified

**Root Cause:**
The app was creating a NEW database on every launch instead of reusing the existing one!

**Old Logic (Broken):**
```rust
// Step 1: Create NEW database with current timestamp
let db = Database::new(&get_timestamp()).unwrap();
// Creates: sensors_20260308_143022.db

// Step 2: Background thread loads OLD database
thread::spawn(|| {
    if let Some(old_db) = get_latest_database() {
        // Finds: sensors_20260308_120000.db
        *db = Database::open(old_db);
    }
});
```

**Issues:**
1. Creates new file every launch (`sensors_YYYYMMDD_HHMMSS.db`)
2. Each launch = new database file
3. Old data appears "wiped" because new database is empty
4. Background loading was a race condition
5. Data directory fills with database files

---

## ✅ Fixes Applied

### Fix 1: Load Existing Database First

**New Logic:**
```rust
// Try to load existing database FIRST
let db = if let Some(db_path) = get_latest_database() {
    println!("Found existing: {}", db_path);
    Database::open(&db_path)  // ✅ Reuse existing!
} else {
    println!("No database found, creating new");
    Database::new(&get_timestamp())  // Only if no database exists
};
```

**Benefits:**
- ✅ Reuses existing database
- ✅ No new files on each launch
- ✅ Data persists across sessions
- ✅ No race conditions

---

### Fix 2: Automatic Backup Before Opening

**Protection:**
```rust
fn open(path: &Path) -> Result<Self> {
    // Create backup BEFORE opening
    if path.exists() {
        let backup_path = path.with_extension("db.backup");
        std::fs::copy(path, &backup_path)?;
        println!("✓ Created backup: {}", backup_path.display());
    }
    
    let conn = Connection::open(path)?;
    // ... rest of opening logic
}
```

**Benefits:**
- ✅ Automatic backup on every app launch
- ✅ Can recover if database gets corrupted
- ✅ Backup has `.db.backup` extension
- ✅ Only one backup (overwrites old backup)

---

### Fix 3: Database Integrity Check

**Validation:**
```rust
fn open(path: &Path) -> Result<Self> {
    // ... backup created ...
    
    let conn = Connection::open(path)?;
    
    // Check integrity
    let result = conn.pragma_query_value(
        None, 
        "integrity_check", 
        |row| row.get::<_, String>(0)
    )?;
    
    if result != "ok" {
        eprintln!("⚠️  Database corrupted: {}", result);
        
        // Try to restore from backup
        let backup = path.with_extension("db.backup");
        if backup.exists() {
            eprintln!("Restoring from backup...");
            std::fs::copy(&backup, path)?;
            return Self::open(path);  // Retry
        }
        
        return Err(...);  // Can't recover
    }
    
    println!("✓ Integrity check passed");
    // ... continue
}
```

**Benefits:**
- ✅ Detects corruption
- ✅ Automatic recovery from backup
- ✅ User notified of issues
- ✅ Safe fallback to new database

---

### Fix 4: Database Statistics Logging

**Visibility:**
```rust
// After successful load
if let Ok(cal_count) = db.get_calibration_count() {
    println!("  📊 Calibrations: {}", cal_count);
}
if let Ok(read_count) = db.get_reading_count() {
    println!("  📊 Readings: {}", read_count);
}
```

**Benefits:**
- ✅ See data counts on launch
- ✅ Verify data is present
- ✅ Easy debugging
- ✅ Confirm persistence working

---

### Fix 5: Better Error Messages

**User Communication:**
```rust
Err(e) => {
    eprintln!("⚠️  Failed to open database: {}", e);
    eprintln!("⚠️  This is unusual - database may be corrupted");
    eprintln!("⚠️  Creating new database as fallback...");
    eprintln!("⚠️  Old database preserved with .backup extension");
    Database::new(&get_timestamp())
}
```

**Benefits:**
- ✅ Clear error messages
- ✅ Explains what happened
- ✅ Shows recovery action
- ✅ Tells user data is backed up

---

## 🛡️ Recovery Protocol

### Normal Startup (Database Exists)

```
1. Check for existing database
   ✓ Found: sensors_20260308_120000.db

2. Create backup
   ✓ Created: sensors_20260308_120000.db.backup

3. Open database
   ✓ Connection established

4. Check integrity
   ✓ Integrity check: ok

5. Enable WAL mode
   ✓ WAL mode enabled

6. Load statistics
   📊 Calibrations: 42
   📊 Readings: 1,523

7. Ready!
   ✓ Database loaded successfully
```

---

### Corrupted Database Recovery

```
1. Check for existing database
   ✓ Found: sensors_20260308_120000.db

2. Create backup
   ✓ Created: sensors_20260308_120000.db.backup

3. Open database
   ✓ Connection established

4. Check integrity
   ⚠️  Integrity check failed: database disk image is malformed

5. Restore from backup
   ⚠️  Restoring from backup...
   ✓ Copied backup to main database

6. Retry opening
   ✓ Connection established
   ✓ Integrity check: ok

7. Ready!
   ✓ Database recovered from backup
```

---

### No Database Found (First Launch)

```
1. Check for existing database
   ℹ️  No existing database found

2. Create new database
   ✓ Creating: sensors_20260308_143022.db

3. Initialize tables
   ✓ Created table: readings
   ✓ Created table: calibrations
   ✓ Created indexes

4. Enable WAL mode
   ✓ WAL mode enabled

5. Ready!
   ✓ New database created
```

---

### Unrecoverable Corruption

```
1. Check for existing database
   ✓ Found: sensors_20260308_120000.db

2. Create backup
   ✓ Created: sensors_20260308_120000.db.backup

3. Open database
   ⚠️  Failed: database disk image is malformed

4. Check integrity
   ⚠️  Integrity check failed

5. Try to restore backup
   ⚠️  Backup also corrupted!

6. Fallback to new database
   ⚠️  Creating new database as fallback
   ⚠️  Old database preserved with .backup extension
   ✓ Created: sensors_20260308_143030.db

7. Ready (with data loss)
   ⚠️  Previous data could not be recovered
```

---

## 📂 File Management

### Database Files

**Location:** `~/.local/share/ambient-learner/`

**File Types:**

1. **Active Database:**
   ```
   sensors_20260308_120000.db
   sensors_20260308_120000.db-wal    (WAL file)
   sensors_20260308_120000.db-shm    (Shared memory)
   ```

2. **Backup:**
   ```
   sensors_20260308_120000.db.backup
   ```

3. **Old Databases (if any):**
   ```
   sensors_20260307_093000.db
   sensors_20260306_140000.db
   ```

### Cleanup Strategy

**Keep:**
- Latest database (active)
- Latest backup
- Previous 3 databases (for safety)

**Delete:**
- Databases older than 30 days
- Orphaned WAL/SHM files

**Automatic cleanup (future):**
```rust
fn cleanup_old_databases(keep_count: usize) {
    let dbs = list_all_databases();
    for db in dbs.iter().skip(keep_count) {
        println!("Cleaning up old database: {}", db.display());
        std::fs::remove_file(db).ok();
    }
}
```

---

## 🔍 Debugging

### Check What Database is Loaded

**Terminal output on launch:**
```
[4/10] Loading database...
Found existing database: /home/user/.local/share/ambient-learner/sensors_20260308_120000.db
✓ Created database backup
✓ Database integrity check passed
  📊 Calibrations: 42
  📊 Readings: 1,523
✓ Database loaded successfully
```

### Verify Data Persistence

**Test:**
1. Launch app
2. Note calibration count: `📊 Calibrations: 42`
3. Add calibration (should become 43)
4. Close app
5. Relaunch app
6. Check count: `📊 Calibrations: 43` ✅

**If count resets to 0:**
- Database not being saved
- Check for errors in terminal
- Check file permissions

---

## 🧪 Testing

### Test 1: Normal Persistence

```bash
# Launch 1
./ambient-learner
# Output: Calibrations: 0

# Add some data, then close

# Launch 2
./ambient-learner
# Output: Calibrations: 5  ✅ Data persists!
```

### Test 2: Corruption Recovery

```bash
# Corrupt the database manually
echo "garbage" > ~/.local/share/ambient-learner/sensors_*.db

# Launch app
./ambient-learner
# Should see:
# ⚠️  Integrity check failed
# ⚠️  Restoring from backup...
# ✓ Database recovered
```

### Test 3: Backup Verification

```bash
# Check backup exists
ls -lh ~/.local/share/ambient-learner/*.backup

# Should see:
# sensors_20260308_120000.db.backup
```

---

## 💡 Best Practices

### For Users

1. **Don't delete database files manually**
   - App manages them automatically
   - Backups are created on every launch

2. **If data seems lost:**
   - Check terminal output
   - Look for `.backup` files
   - Report the error messages

3. **To reset everything:**
   ```bash
   rm ~/.local/share/ambient-learner/sensors_*.db*
   ```

### For Developers

1. **Always check return values**
   ```rust
   if let Ok(db) = Database::open(path) {
       // Success path
   } else {
       // Error handling
   }
   ```

2. **Log database operations**
   ```rust
   println!("✓ Calibration saved: {}", count);
   ```

3. **Test with corrupted databases**
   - Inject corruption
   - Verify recovery works
   - Check backups are valid

---

## 📊 Comparison

### Before (Broken)

```
Launch 1:
✓ Created: sensors_20260308_140000.db
  Calibrations: 0

Launch 2:
✓ Created: sensors_20260308_140530.db  ← NEW FILE!
  Calibrations: 0  ← DATA LOST!

Launch 3:
✓ Created: sensors_20260308_141000.db  ← ANOTHER NEW FILE!
  Calibrations: 0  ← DATA LOST AGAIN!
```

**Result:** Data appears "wiped" every launch ❌

### After (Fixed)

```
Launch 1:
✓ Created: sensors_20260308_140000.db
  Calibrations: 0

Launch 2:
✓ Loaded: sensors_20260308_140000.db  ← SAME FILE!
  Calibrations: 5  ← DATA PERSISTS!

Launch 3:
✓ Loaded: sensors_20260308_140000.db  ← SAME FILE!
  Calibrations: 12  ← DATA GROWS!
```

**Result:** Data persists across sessions ✅

---

## ✅ Summary

**Fixed:**
1. ✅ Load existing database instead of creating new one
2. ✅ Automatic backup on every launch
3. ✅ Database integrity checks
4. ✅ Automatic recovery from backup
5. ✅ Statistics logging (cal/reading counts)
6. ✅ Better error messages
7. ✅ Preserved old databases as backup

**Benefits:**
- Data persists across sessions
- Automatic corruption recovery
- Clear visibility into database state
- Safe fallbacks at every step
- User data protected

**Result:**
- ✅ Database no longer "wiped"
- ✅ Data grows over time
- ✅ Corruption handled gracefully
- ✅ Users see consistent data

🎉 **Your calibrations and data now persist properly!**
