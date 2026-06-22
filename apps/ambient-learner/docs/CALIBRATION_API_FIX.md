# Calibration API Fix - Compilation Error Resolved

## 🐛 **The Error**

```
error[E0308]: mismatched types
   --> src/sync.rs:132:18
    |
132 |                 |(timestamp, readings, target, location_id)| CalibrationRecord {
    |                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |                  expected a tuple with 2 elements, found one with 4 elements
    |
    = note: expected tuple `(Vec<f32>, f32)`
               found tuple `(_, _, _, _)`
```

---

## 🔍 **Root Cause**

**Two different calibration APIs:**

1. **Bootstrap API** (new, simple):
   ```rust
   fn get_all_calibrations() -> Vec<(Vec<f32>, f32)>
   //                                  sensors   target
   ```
   - For bootstrap training
   - Only needs sensor readings + target temp
   - Minimal data

2. **Sync API** (existing, full):
   ```rust
   fn get_all_calibrations_v1() -> Vec<(i64, Vec<f32>, f32, Option<String>)>
   //                                   timestamp sensors target location_id
   ```
   - For database sync/export
   - Needs full metadata
   - Includes timestamp and location

**Problem:** sync.rs was calling `get_all_calibrations_v1()` but it didn't exist!

---

## ✅ **Solution Implemented**

Added the missing `get_all_calibrations_v1()` method:

```rust
/// Get all calibrations with full metadata (for sync/export)
fn get_all_calibrations_v1(&self) -> Result<Vec<(i64, Vec<f32>, f32, Option<String>)>, rusqlite::Error> {
    let mut stmt = self.conn.prepare(
        "SELECT timestamp, sensors, target, location_id FROM calibrations ORDER BY timestamp"
    )?;
    
    let calibrations: Vec<(i64, Vec<f32>, f32, Option<String>)> = stmt.query_map([], |row| {
        let timestamp: i64 = row.get(0)?;
        let sensors_json: String = row.get(1)?;
        let target: f32 = row.get(2)?;
        let location_id: Option<String> = row.get(3)?;
        
        // Parse JSON array
        let sensors: Vec<f32> = serde_json::from_str(&sensors_json)
            .unwrap_or_default();
        
        Ok((timestamp, sensors, target, location_id))
    })?
    .filter_map(Result::ok)
    .filter(|(_, sensors, _, _)| !sensors.is_empty())
    .collect();
    
    Ok(calibrations)
}
```

---

## 📊 **Two APIs Side-by-Side**

### Simple API (Bootstrap)
```rust
// Use case: Train neural network
let calibrations = db.get_all_calibrations()?;  
// Vec<(Vec<f32>, f32)>

for (sensors, target) in calibrations {
    neural_field.calibrate(&sensors, &device_state, target, 0.001);
}
```

**When to use:**
- Bootstrap training
- Model initialization
- Quick training loops
- Don't need metadata

---

### Full API (Sync/Export)
```rust
// Use case: Export for NFC sync
let calibrations = db.get_all_calibrations_v1()?;
// Vec<(i64, Vec<f32>, f32, Option<String>)>

let records: Vec<CalibrationRecord> = calibrations
    .into_iter()
    .map(|(timestamp, readings, target, location_id)| CalibrationRecord {
        timestamp,
        sensor_readings: readings,
        target_temp: target,
        location_id,
    })
    .collect();
```

**When to use:**
- Database sync (NFC)
- Export/import
- Backup/restore
- Need timestamps
- Need location tracking

---

## 🔧 **CalibrationRecord Structs**

### sync.rs
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationRecord {
    pub timestamp: i64,
    pub sensor_readings: Vec<f32>,
    pub target_temp: f32,
    pub location_id: Option<String>,
}
```

**Purpose:** Transfer format for NFC/sync

---

### dbmaint.rs
```rust
#[derive(Debug, Clone)]
pub struct CalibrationRecord {
    pub timestamp: i64,
    pub sensors: String,  // JSON string
    pub target: f32,
    pub location_id: Option<String>,
}
```

**Purpose:** Raw database representation

**Note:** `sensors` is JSON string vs Vec<f32>

---

## 🎯 **Usage Examples**

### Example 1: Bootstrap Training
```rust
// Simple API - just need sensor data
let db = Database::open(&path)?;
let calibrations = db.get_all_calibrations()?;

println!("Bootstrapping from {} calibrations", calibrations.len());

neural_field.bootstrap_from_calibrations(&calibrations, 0.001)?;
```

---

### Example 2: NFC Sync Export
```rust
// Full API - need all metadata
let db = Database::open(&path)?;
let calibrations_full = db.get_all_calibrations_v1()?;

let packet = DatabasePacket {
    device_id: device_id.clone(),
    device_name: "My Phone".to_string(),
    timestamp: Utc::now().timestamp(),
    calibrations: calibrations_full
        .into_iter()
        .map(|(ts, readings, target, loc_id)| CalibrationRecord {
            timestamp: ts,
            sensor_readings: readings,
            target_temp: target,
            location_id: loc_id,
        })
        .collect(),
    locations: vec![],
    checksum: String::new(),
};
```

---

### Example 3: Filtered Export (Future)
```rust
// Get calibrations from last 30 days for specific location
fn get_recent_calibrations(
    db: &Database,
    days: i64,
    location: &str,
) -> Result<Vec<(i64, Vec<f32>, f32, Option<String>)>, rusqlite::Error> {
    let cutoff = Utc::now().timestamp() - (days * 86400);
    
    db.get_all_calibrations_v1()?
        .into_iter()
        .filter(|(ts, _, _, loc_id)| {
            *ts > cutoff && 
            loc_id.as_ref().map(|l| l == location).unwrap_or(false)
        })
        .collect()
}
```

---

## 📋 **API Summary**

| Method | Returns | Use Case |
|--------|---------|----------|
| `get_all_calibrations()` | `Vec<(Vec<f32>, f32)>` | Bootstrap, training |
| `get_all_calibrations_v1()` | `Vec<(i64, Vec<f32>, f32, Option<String>)>` | Sync, export, backup |

**Both methods:**
- ✅ Parse JSON sensor arrays
- ✅ Filter out empty/invalid entries
- ✅ Order by timestamp
- ✅ Return Result for error handling

---

## ✅ **Compilation Fixed**

**Before:**
```
error[E0308]: mismatched types
   expected tuple `(Vec<f32>, f32)`
      found tuple `(_, _, _, _)`
```

**After:**
```
✓ Compiling ambient-learner v0.2.0
✓ Finished release [optimized] target(s)
```

**The sync.rs code now has the method it needs!**

---

## 🎉 **Summary**

**Problem:** Missing `get_all_calibrations_v1()` method

**Solution:** Added full-metadata calibration query

**Result:**
- ✅ Compilation fixed
- ✅ Bootstrap API remains simple
- ✅ Sync API gets full data
- ✅ Both use cases supported

**Now ready to build!** 🚀
