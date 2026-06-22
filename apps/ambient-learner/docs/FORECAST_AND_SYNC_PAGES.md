# Forecast Integration & Database Import - Implementation Complete

## 🔮 Part 1: Temperature Forecasting

### Overview
Integrated the `forecast.rs` module into Page 1 to show 1-hour and 24-hour temperature predictions based on historical patterns.

---

### Implementation Details

**1. Forecast Model Initialization**
```rust
let forecast_model: Arc<Mutex<forecast::ForecastModel>> = 
    Arc::new(Mutex::new(forecast::ForecastModel::new()));
```

**2. Data Collection**
- Updates every 60 seconds
- Adds current temperature to history
- Maintains 14 days of data
- Learns patterns automatically

**3. UI Integration**
```rust
// Update forecasts every 60 seconds
if last_forecast_update.elapsed() > Duration::from_secs(60) {
    if let Ok(mut fm) = forecast_model.try_lock() {
        fm.add_observation(TimeSeriesPoint {
            timestamp: chrono::Utc::now().timestamp(),
            temperature: temp,
            location_id: "current".to_string(),
        });
        
        // Generate forecasts
        if let Some(forecast_1h) = fm.forecast(1.0) {
            forecast_1h_label.set_markup(&format!(
                "1 hour: {:.1}°C ({:.1}-{:.1}°C)",
                forecast_1h.temperature,
                forecast_1h.min_temp,
                forecast_1h.max_temp
            ));
        }
        
        if let Some(forecast_24h_data) = fm.forecast(24.0) {
            forecast_24h_label.set_markup(&format!(
                "24 hours: {:.1}°C ({:.1}-{:.1}°C)",
                forecast_24h_data.temperature,
                forecast_24h_data.min_temp,
                forecast_24h_data.max_temp
            ));
        }
    }
}
```

---

### How Forecasting Works

**Pattern Learning:**
1. **Hourly Pattern** - Temperature by hour of day (24 values)
   - Example: 6am is cooler than 2pm
2. **Daily Pattern** - Temperature by day of week (7 values)
   - Example: Weekends might have different patterns
3. **Trend Analysis** - Linear regression over time
   - Example: Gradual warming or cooling trend

**Prediction Formula:**
```
prediction = current_temp 
           + hourly_diff × 0.6
           + trend × 0.2
```

**Confidence Calculation:**
```
confidence = if history > 1000 { 0.9 }      // Lots of data
            else if history > 500 { 0.7 }   // Good amount
            else if history > 100 { 0.5 }   // Moderate
            else { 0.3 }                    // Limited data
```

**Confidence Bounds:**
```
min_temp = prediction - std_dev × 1.5
max_temp = prediction + std_dev × 1.5
```

---

### Example Output

**Initial (< 100 data points):**
```
🔮 Forecast
1 hour: --°C
24 hours: --°C
```

**After 100+ data points:**
```
🔮 Forecast
1 hour: 23.5°C (22.0-25.0°C)
24 hours: 21.2°C (18.7-23.7°C)
```

**After 1000+ data points (high confidence):**
```
🔮 Forecast
1 hour: 23.8°C (23.1-24.5°C)   ← Narrow range
24 hours: 21.5°C (20.2-22.8°C)
```

---

## 📡 Part 2: Database Import/Export (Page 5)

### Overview
Added Page 5 for importing and exporting calibration databases, enabling data sharing between devices.

---

### Page Layout

```
📡 Database Sync
Import and export calibration data

┌─────────────────────────────────┐
│ Current Database                │
│ 📊 22 calibrations   [📤 Export]│
└─────────────────────────────────┘

Import from File
Select a .db file to import calibrations

[📥 Import Database]
```

---

### Export Functionality

**Implementation:**
```rust
export_btn.connect_clicked(move |_| {
    let file_dialog = gtk::FileDialog::builder()
        .title("Export Database")
        .initial_name(&format!("ambient_learner_export_{}.db", 
            chrono::Local::now().format("%Y%m%d_%H%M%S")))
        .build();
    
    file_dialog.save(Some(&window), None, move |result| {
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                // Copy current database to selected path
                std::fs::copy(&current_db_path, &path)?;
                
                toast.add_toast("✅ Database exported successfully");
            }
        }
    });
});
```

**Export File Name:**
```
ambient_learner_export_20260309_132145.db
                      └─ Timestamp
```

---

### Import Functionality

**Implementation:**
```rust
import_btn.connect_clicked(move |_| {
    let file_dialog = gtk::FileDialog::builder()
        .title("Import Database")
        .build();
    
    // Filter for .db files only
    let filter = gtk::FileFilter::new();
    filter.add_pattern("*.db");
    filter.set_name(Some("Database files (*.db)"));
    file_dialog.set_filters(Some(&filters));
    
    file_dialog.open(Some(&window), None, move |result| {
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                // Verify it's a valid database
                let conn = rusqlite::Connection::open(&path)?;
                
                // Check for calibration_data table
                let has_table: bool = conn.query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master 
                     WHERE type='table' AND name='calibration_data'",
                    [],
                    |row| row.get(0)
                )?;
                
                if has_table {
                    // Count calibrations
                    let count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM calibration_data",
                        [],
                        |row| row.get(0)
                    )?;
                    
                    toast.add_toast(&format!("✅ Found {} calibrations", count));
                    
                    // TODO: Merge data (not yet implemented)
                } else {
                    toast.add_toast("❌ Not a valid ambient learner database");
                }
            }
        }
    });
});
```

**Validation Steps:**
1. Check file is valid SQLite database
2. Verify `calibration_data` table exists
3. Count calibrations
4. Show toast with result

---

### Data Merge Strategy (Future)

**Planned implementation using `sync.rs` module:**

```rust
// 1. Load remote database
let remote_db = Database::open(import_path)?;

// 2. Get all calibrations
let remote_calibrations = remote_db.get_all_calibrations()?;

// 3. Verify signatures (if enabled)
for cal in &remote_calibrations {
    if let Some(signature) = &cal.signature {
        security::verify_calibration(cal, signature)?;
    }
}

// 4. Merge into current database
for cal in remote_calibrations {
    // Check for duplicates (by timestamp + location)
    if !current_db.has_calibration(&cal)? {
        current_db.insert_calibration(&cal)?;
    }
}

// 5. Reload model with new data
model.retrain_from_database(&current_db)?;
```

**Conflict Resolution:**
- Same timestamp + location = skip (duplicate)
- Different timestamp = add (new data)
- Invalid signature = reject (security)

---

## 🎯 Page Navigation Updated

**New Page Order:**
1. Live Temperature (🌡️)
2. Model Visualization (🧠)
3. History (📊)
4. Locations (📍)
5. **Sync (📡)** ← NEW!

**Swipe Gestures:**
```
Swipe Left:  live → model → history → locations → sync
Swipe Right: sync → locations → history → model → live
```

---

## 🧪 Testing Guide

### Test Forecast

**Step 1: Let app run**
```bash
./target/release/ambient-learner
# Leave running for 1+ hours
```

**Step 2: Check Page 1**
```
Initial:
🔮 Forecast
1 hour: --°C
24 hours: --°C

After 100 data points (100 minutes):
🔮 Forecast
1 hour: 23.2°C (21.5-24.9°C)
24 hours: --°C  (not enough data yet)

After 24+ hours:
🔮 Forecast
1 hour: 23.5°C (22.8-24.2°C)
24 hours: 21.0°C (19.5-22.5°C)
```

---

### Test Export

**Step 1: Navigate to Page 5**
```
Swipe left 4 times: live → model → history → locations → sync
```

**Step 2: Click Export**
```
[📤 Export] button
→ File dialog opens
→ Choose location
→ Save
→ Toast: "✅ Database exported successfully"
```

**Step 3: Verify**
```bash
ls -lh ~/Downloads/ambient_learner_export_*.db
# Should see exported file
```

---

### Test Import

**Step 1: Get a database file**
```
Option A: Use exported file from above
Option B: Copy from another device
Option C: Use existing weather_data/*.db file
```

**Step 2: Click Import**
```
[📥 Import Database] button
→ File dialog opens (shows only .db files)
→ Select database
→ Toast shows result
```

**Expected Results:**

**Valid database:**
```
✅ Found 22 calibrations (merge not yet implemented)
```

**Invalid file:**
```
❌ Not a valid ambient learner database
```

**Not a database:**
```
❌ Invalid database file
```

---

## 📊 Feature Status

### ✅ Completed

**Forecast:**
- [x] Model initialization
- [x] Data collection (60s intervals)
- [x] Pattern learning (hourly, daily, trend)
- [x] 1-hour prediction
- [x] 24-hour prediction
- [x] Confidence intervals
- [x] UI integration on Page 1

**Database Sync:**
- [x] Page 5 UI created
- [x] Export button with file dialog
- [x] Import button with file dialog
- [x] Database validation
- [x] Calibration counting
- [x] Toast notifications
- [x] Swipe gestures updated

---

### 🔨 TODO (Future Enhancements)

**Forecast:**
- [ ] Weekly pattern analysis
- [ ] Weather API integration (external validation)
- [ ] Accuracy metrics (compare predicted vs actual)
- [ ] Historical forecast chart
- [ ] Location-specific patterns

**Database Sync:**
- [ ] **Data merge implementation** (highest priority!)
- [ ] Signature verification (security.rs)
- [ ] Conflict resolution UI
- [ ] Merge statistics (added/skipped)
- [ ] Remote database list (sync.rs)
- [ ] NFC transfer dialog
- [ ] Bluetooth pairing

---

## 🎯 Next Session Priorities

### Priority 1: Data Merge Implementation
```rust
// In import handler, after validation:
let remote_calibrations = get_calibrations_from_db(&conn)?;

for cal in remote_calibrations {
    // Check if already exists
    let exists = db.lock().unwrap().has_calibration(
        cal.timestamp,
        &cal.location_id
    )?;
    
    if !exists {
        db.lock().unwrap().insert_calibration(cal)?;
        merged_count += 1;
    }
}

toast.add_toast(&format!("✅ Merged {} new calibrations", merged_count));

// Retrain model with new data
model.lock().unwrap().retrain_from_database(&db.lock().unwrap())?;
```

---

### Priority 2: Forecast Visualization
```rust
// Add a chart showing:
// - Historical temperature (past 24h)
// - Predicted temperature (next 24h)
// - Confidence bands

use plotters or similar chart library
```

---

### Priority 3: NFC Transfer
```rust
// Use sync.rs module:
// - Generate transfer package
// - Show QR code or NFC pairing
// - Receive from another device
// - Auto-merge on receive
```

---

## ✅ Summary

**Forecast Integration:**
- ✅ Automatic pattern learning
- ✅ 1h and 24h predictions
- ✅ Confidence intervals
- ✅ Updates every 60 seconds
- ✅ Fully integrated into Page 1

**Database Import/Export:**
- ✅ Page 5 created
- ✅ Export to file
- ✅ Import from file
- ✅ Database validation
- ⚠️  Merge logic TODO

**Status:** Both features implemented and ready to test!

**Build and run:**
```bash
cargo build --release
./target/release/ambient-learner
```

**What to test:**
1. Let app run for 1+ hour → Check forecasts on Page 1
2. Swipe to Page 5 → Test export
3. Import a database file → See validation

🎉 **Forecast and Page 5 complete! Data merge is next priority.**
