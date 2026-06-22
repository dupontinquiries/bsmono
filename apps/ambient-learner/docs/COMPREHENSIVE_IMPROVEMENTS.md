# Comprehensive Improvements: Sensors, Database & QT GUI

## 🎯 **Part 1: Sensor Learning Improvements**

### **Issue 1: Only 2 Active Sensors (CRITICAL)**

**Apply These Fixes:**

#### **Fix 1.1: Gentler Dropout Score Formula**
**File:** `src/main.rs` **Line:** 626

```rust
// OLD (too harsh):
stat.dropout_score = 1.0 / (1.0 + stat.variance);

// NEW (gentler - keeps more sensors):
stat.dropout_score = 1.0 / (1.0 + stat.variance.sqrt());
```

---

#### **Fix 1.2: Lower Dropout Threshold**
**File:** `src/main.rs` **Line:** 267

```rust
// OLD:
pub const MIN_DROPOUT_SCORE: f32 = 0.1;

// NEW:
pub const MIN_DROPOUT_SCORE: f32 = 0.05;
```

---

#### **Fix 1.3: Add Top 10% Keep Rule**
**File:** `src/main.rs` **After line:** 1243

Add call in `calibrate_linear`:
```rust
fn calibrate_linear(&mut self, readings: &[f32], target: f32) {
    let prediction = self.predict(readings);
    let error = target - prediction;

    self.bias += LEARNING_RATE * error;

    for (i, &val) in readings.iter().enumerate() {
        if i >= self.stats.len() {
            break;
        }
        let stat = &mut self.stats[i];

        let std_dev = stat.variance.sqrt().max(0.001);
        let normalized = (val - stat.mean) / std_dev;
        let gradient = error * normalized / (normalized * normalized + 1e-6);
        stat.weight += LEARNING_RATE * gradient - 0.001 * stat.weight;
    }
    
    // NEW: Apply top-k rule after weight updates
    if self.calibration_count >= 5 {
        self.apply_top_k_rule();
    }
}
```

---

#### **Fix 1.4: Add Top-K Rule Function**
**File:** `src/main.rs` **After line:** 1259

```rust
/// Keep top K% of sensors active based on absolute weight
/// Ensures at least min_keep sensors are always active
fn apply_top_k_rule(&mut self) {
    let num_sensors = self.stats.len();
    let min_keep = 5; // Always keep at least 5 sensors
    let keep_percent = 10; // Keep top 10%
    let top_k = ((num_sensors * keep_percent) / 100).max(min_keep);
    
    // Build (index, abs_weight) pairs
    let mut indexed_weights: Vec<(usize, f32)> = self.stats
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.weight.abs()))
        .collect();
    
    // Sort by weight descending
    indexed_weights.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    
    // Boost top-k sensors (but cap at 1.0)
    for &(idx, _) in indexed_weights.iter().take(top_k) {
        self.stats[idx].dropout_score = (self.stats[idx].dropout_score * 1.2).min(1.0);
    }
    
    // Gently reduce dropout for weak sensors (only if already low)
    for &(idx, _) in indexed_weights.iter().skip(top_k) {
        if self.stats[idx].dropout_score < 0.2 {
            // Very gentle reduction
            self.stats[idx].dropout_score *= 0.95;
        }
    }
    
    // Debug logging every 5 calibrations
    if self.calibration_count % 5 == 0 {
        let active = self.stats.iter()
            .filter(|s| s.dropout_score > MIN_DROPOUT_SCORE)
            .count();
        
        let top_sensors: Vec<String> = indexed_weights.iter()
            .take(3)
            .map(|(idx, weight)| format!("S{}: {:.3}", idx, weight))
            .collect();
        
        println!("📊 Active sensors: {} / {} (keeping top {})", 
            active, num_sensors, top_k);
        println!("   Top contributors: {}", top_sensors.join(", "));
    }
}
```

---

#### **Fix 1.5: Remove Harsh Pruning**
**File:** `src/main.rs` **Lines:** 1246-1259

```rust
// COMMENT OUT or DELETE this entire function:
/*
fn prune_weak_sensors(&mut self) {
    if self.calibration_count > 5 {
        for stat in &mut self.stats {
            if stat.weight.abs() < 0.01 {
                stat.dropout_score *= 0.5;  // Too harsh!
            }
        }
    }
    
    if self.calibration_count == 10 || self.calibration_count == 20 {
        self.detect_material_type();
    }
}
*/

// NEW (gentler version):
fn prune_weak_sensors(&mut self) {
    // Only run material detection, skip harsh pruning
    if self.calibration_count == 10 || self.calibration_count == 20 {
        self.detect_material_type();
    }
}
```

---

## 🗄️ **Part 2: Database Saving Fixes**

### **Issue 2.1: Readings Only Saved Every Hour**

**File:** `src/main.rs` **Line:** 3500

```rust
// PROBLEM: 3600 seconds = 1 hour between saves!
if last_save.elapsed().as_secs() >= 3600 {
```

**Fix - Make Configurable:**

Add to constants (line ~260):
```rust
pub mod database {
    /// How often to save sensor readings (seconds)
    pub const READING_SAVE_INTERVAL_SECS: u64 = 300;  // 5 minutes instead of 1 hour
    
    /// Minimum change required to save reading (prevents duplicate saves)
    pub const MIN_READING_CHANGE_THRESHOLD: f32 = 0.5;  // 0.5°C change
}
```

Update the save logic:
```rust
// Line 3500 - Update interval
if last_save.elapsed().as_secs() >= database::READING_SAVE_INTERVAL_SECS {
    // ... rest of logic
}
```

---

### **Issue 2.2: Hash-Based Deduplication Too Aggressive**

**File:** `src/main.rs` **Lines:** 3505-3512

```rust
// PROBLEM: Rounds to 0.01°C precision, may miss small changes
let mut hasher = DefaultHasher::new();
for &val in &readings {
    ((val * 100.0) as i32).hash(&mut hasher);  // 0.01°C precision
}
```

**Fix - Add Temperature Change Threshold:**

```rust
// Better approach: Check if temperature meaningfully changed
fn has_reading_changed(old_readings: &[f32], new_readings: &[f32], threshold: f32) -> bool {
    if old_readings.len() != new_readings.len() {
        return true;
    }
    
    // Check if average temperature changed by threshold
    let old_avg: f32 = old_readings.iter().sum::<f32>() / old_readings.len() as f32;
    let new_avg: f32 = new_readings.iter().sum::<f32>() / new_readings.len() as f32;
    
    (new_avg - old_avg).abs() > threshold
}

// Usage (replace hash check):
if has_reading_changed(&last_readings, &readings, database::MIN_READING_CHANGE_THRESHOLD) {
    // Save reading
    // ...
    last_readings = readings.clone();
}
```

---

### **Issue 2.3: Calibrations May Not Save Due to Lock Failures**

**File:** `src/main.rs` **Line:** 4560

```rust
match db.store_calibration(&avg_reading, target, location_id.as_deref()) {
    Ok(_) => {
        println!("✅ Calibration saved to database");
        // ... update model ...
    }
    Err(e) => eprintln!("Failed to save calibration: {}", e),
}
```

**Problem:** Error is logged but calibration is lost!

**Fix - Add Retry Queue:**

```rust
use std::sync::mpsc;

// Create calibration queue channel
let (cal_tx, cal_rx) = mpsc::channel::<(Vec<f32>, f32, Option<String>)>();

// In calibration button handler:
match db.store_calibration(&avg_reading, target, location_id.as_deref()) {
    Ok(_) => {
        println!("✅ Calibration saved to database");
    }
    Err(e) => {
        eprintln!("⚠️  Failed to save calibration immediately: {}", e);
        eprintln!("   Adding to retry queue...");
        
        // Queue for retry
        let _ = cal_tx.send((
            avg_reading.clone(),
            target,
            location_id.clone()
        ));
    }
}

// Background thread to retry failed calibrations
let db_retry = db.clone();
std::thread::spawn(move || {
    while let Ok((sensors, target, loc)) = cal_rx.recv() {
        for attempt in 0..5 {
            std::thread::sleep(std::time::Duration::from_secs(2_u64.pow(attempt)));
            
            if let Ok(db) = db_retry.lock() {
                match db.store_calibration(&sensors, target, loc.as_deref()) {
                    Ok(_) => {
                        println!("✅ Queued calibration saved on retry {}", attempt + 1);
                        break;
                    }
                    Err(e) => {
                        eprintln!("⚠️  Retry {} failed: {}", attempt + 1, e);
                    }
                }
            }
        }
    }
});
```

---

### **Issue 2.4: Missing Indexes (From Previous Analysis)**

**File:** `src/main.rs` **Line:** 1627

```rust
conn.execute("CREATE INDEX IF NOT EXISTS idx_readings_time ON readings(timestamp)", [])?;
conn.execute("CREATE INDEX IF NOT EXISTS idx_readings_location ON readings(location_id)", [])?;

// ADD THESE:
conn.execute("CREATE INDEX IF NOT EXISTS idx_calibrations_time ON calibrations(timestamp)", [])?;
conn.execute("CREATE INDEX IF NOT EXISTS idx_calibrations_location ON calibrations(location_id)", [])?;
```

---

### **Issue 2.5: Broken get_calibration_history (From Previous Analysis)**

**File:** `src/main.rs` **Line:** 1971

```rust
// BROKEN: Column is 'target' not 'target_temp'!
let query = "SELECT timestamp, target_temp 
             FROM calibrations 
             WHERE timestamp > ? 
             ORDER BY timestamp ASC";

// FIX:
let query = "SELECT timestamp, target   -- Column is 'target' not 'target_temp'
             FROM calibrations 
             WHERE timestamp > ? 
             ORDER BY timestamp ASC";
```

---

## 📱 **Part 3: QT/QML GUI Implementation**

### **Issue 3.1: QT Backend Incomplete**

**File:** `src/qt_backend.rs` is minimal - needs full implementation

#### **Step 1: Create QML UI File**

**File:** `src/ui/main.qml` (NEW)

```qml
import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

ApplicationWindow {
    id: window
    visible: true
    width: 400
    height: 700
    title: "Ambient Learner"
    
    // Access Rust backend
    property var backend: QtBackend
    
    // Update timer
    Timer {
        interval: 100  // 10 FPS
        running: true
        repeat: true
        onTriggered: backend.poll_updates()
    }
    
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 20
        
        // Header
        Label {
            text: "🌡️ Ambient Temperature"
            font.pixelSize: 24
            font.bold: true
            Layout.alignment: Qt.AlignHCenter
        }
        
        // Main temperature display
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 200
            radius: 20
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#1a1a2e" }
                GradientStop { position: 1.0; color: "#16213e" }
            }
            
            ColumnLayout {
                anchors.centerIn: parent
                spacing: 10
                
                Label {
                    text: backend.temperature.toFixed(1) + "°C"
                    font.pixelSize: 72
                    font.bold: true
                    color: getTempColor(backend.temperature)
                    Layout.alignment: Qt.AlignHCenter
                }
                
                // Confidence bar
                ProgressBar {
                    Layout.preferredWidth: 200
                    Layout.alignment: Qt.AlignHCenter
                    value: backend.confidence
                    from: 0
                    to: 1
                }
                
                Label {
                    text: "Confidence: " + (backend.confidence * 100).toFixed(0) + "%"
                    font.pixelSize: 14
                    color: "#888"
                    Layout.alignment: Qt.AlignHCenter
                }
            }
        }
        
        // Model info
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 100
            radius: 15
            color: "#1a1a2e"
            
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 15
                spacing: 8
                
                Label {
                    text: "Model: " + backend.model_type
                    font.pixelSize: 16
                    color: "#fff"
                }
                
                Label {
                    text: "Calibrations: " + backend.calibration_count
                    font.pixelSize: 14
                    color: "#aaa"
                }
                
                Label {
                    text: "Sensors: " + backend.sensor_count
                    font.pixelSize: 14
                    color: "#aaa"
                }
            }
        }
        
        // Calibration section
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 150
            radius: 15
            color: "#1a1a2e"
            
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 15
                spacing: 10
                
                Label {
                    text: "Calibrate"
                    font.pixelSize: 18
                    font.bold: true
                    color: "#fff"
                }
                
                RowLayout {
                    spacing: 10
                    
                    TextField {
                        id: targetTempField
                        Layout.fillWidth: true
                        placeholderText: "Enter actual temperature..."
                        validator: DoubleValidator { bottom: -50; top: 100 }
                        font.pixelSize: 16
                    }
                    
                    Label {
                        text: "°C"
                        font.pixelSize: 16
                        color: "#aaa"
                    }
                }
                
                Button {
                    Layout.fillWidth: true
                    text: "📍 Calibrate"
                    font.pixelSize: 16
                    enabled: targetTempField.text.length > 0
                    onClicked: {
                        backend.calibrate(parseFloat(targetTempField.text))
                        targetTempField.text = ""
                    }
                    
                    background: Rectangle {
                        radius: 8
                        color: parent.enabled ? "#0077ff" : "#444"
                    }
                    
                    contentItem: Text {
                        text: parent.text
                        font: parent.font
                        color: "#fff"
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                }
            }
        }
        
        // Location
        Label {
            text: "📍 " + backend.location
            font.pixelSize: 14
            color: "#888"
            Layout.alignment: Qt.AlignHCenter
        }
        
        Item { Layout.fillHeight: true }  // Spacer
    }
    
    // Helper function for temperature color
    function getTempColor(temp) {
        if (temp < 10) return "#64b5f6"  // Cold - blue
        if (temp < 18) return "#81c784"  // Cool - green
        if (temp < 24) return "#fff176"  // Moderate - yellow
        if (temp < 30) return "#ffb74d"  // Warm - orange
        return "#ef5350"  // Hot - red
    }
}
```

---

#### **Step 2: Expand QT Backend**

**File:** `src/qt_backend.rs` - ADD MORE METHODS:

```rust
#[cfg(feature = "qt")]
impl QtBackend {
    // ... existing code ...
    
    /// Calibrate - called from QML
    fn calibrate(&mut self, target_temp: f32) -> bool {
        println!("🎯 Qt calibration requested: {}°C", target_temp);
        
        if target_temp < -50.0 || target_temp > 100.0 {
            println!("❌ Invalid temperature range");
            return false;
        }
        
        if let Some(ref state) = self.world_state {
            if let Ok(mut world) = state.write() {
                // Trigger calibration
                world.flags.model = true;
                world.pending_calibration = Some(target_temp);
                return true;
            }
        }
        
        false
    }
    
    /// Export data to CSV
    fn export_csv(&self, path: QString) -> bool {
        println!("📤 Export requested: {}", path.to_string());
        
        // TODO: Implement CSV export
        // For now return success
        true
    }
    
    /// Get sensor readings as formatted JSON
    fn get_sensor_data(&self) -> QString {
        if let Some(ref state) = self.world_state {
            if let Ok(world) = state.read() {
                let data = serde_json::json!({
                    "count": world.sensor_count,
                    "values": world.sensors,
                    "timestamp": chrono::Utc::now().timestamp()
                });
                
                return serde_json::to_string_pretty(&data)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into();
            }
        }
        "{}".into()
    }
    
    /// Get calibration history
    fn get_history(&self, days: i32) -> QString {
        // TODO: Implement history retrieval from database
        "[]".into()
    }
    
    /// Clear all calibrations (with confirmation)
    fn clear_calibrations(&mut self) -> bool {
        println!("🗑️  Clear calibrations requested");
        
        if let Some(ref state) = self.world_state {
            if let Ok(mut world) = state.write() {
                world.calibration_count = 0;
                return true;
            }
        }
        
        false
    }
}
```

---

#### **Step 3: Add QT Feature to Cargo.toml**

**File:** `Cargo.toml`

```toml
[features]
default = ["gtk"]
gtk = ["gtk4", "libadwaita"]
iced = ["iced"]
qt = ["qmetaobject", "qttypes"]  # NEW

[dependencies]
# ... existing dependencies ...

# QT dependencies (optional)
qmetaobject = { version = "0.2", optional = true }
qttypes = { version = "0.2", optional = true }
```

---

#### **Step 4: Add QT Entry Point**

**File:** `src/main.rs` - ADD QT UI FUNCTION:

```rust
#[cfg(feature = "qt")]
fn run_qt_ui() {
    use qmetaobject::*;
    use crate::qt_backend::QtBackend;
    
    println!("🎨 Launching Qt/QML UI...");
    
    // Initialize backend
    qml_register_type::<QtBackend>(
        cstr::cstr!("Backend"),
        1, 0,
        cstr::cstr!("QtBackend")
    );
    
    // Load QML
    let mut engine = QmlEngine::new();
    engine.load_file("src/ui/main.qml".into());
    
    // Get backend instance
    if let Some(backend) = engine.get_property("backend") {
        // Attach world state
        // ... connection code ...
    }
    
    engine.exec();
}

#[cfg(not(feature = "qt"))]
fn run_qt_ui() {
    eprintln!("❌ Qt UI not compiled in");
    std::process::exit(1);
}
```

---

## 📋 **Implementation Checklist**

### **Sensors (High Priority):**
- [ ] Fix dropout score formula (line 626)
- [ ] Lower MIN_DROPOUT_SCORE (line 267)
- [ ] Add top_k_rule call (line 1243)
- [ ] Add apply_top_k_rule() function (after line 1259)
- [ ] Comment out harsh prune_weak_sensors (lines 1246-1259)
- [ ] Test: Should see 6-10 active sensors instead of 2

### **Database (Critical):**
- [ ] Reduce save interval from 3600s to 300s (line 3500)
- [ ] Add database constants for intervals
- [ ] Fix get_calibration_history column name (line 1971)
- [ ] Add missing indexes (line 1627)
- [ ] Add calibration retry queue
- [ ] Test: Verify readings save every 5 minutes

### **QT GUI (Medium Priority):**
- [ ] Create src/ui/main.qml
- [ ] Expand qt_backend.rs with methods
- [ ] Add qt feature to Cargo.toml
- [ ] Add run_qt_ui() to main.rs
- [ ] Test: `cargo build --features qt`
- [ ] Test: `cargo run --features qt`

---

## 🧪 **Testing Commands**

### **Test Sensor Fixes:**
```bash
cargo build --release
./target/release/ambient-learner

# Watch console for:
📊 Active sensors: X / 53 (keeping top Y)
   Top contributors: S23: 0.456, S12: 0.234, S7: 0.123

# Should see 6-10 active sensors
```

### **Test Database Fixes:**
```bash
# Check database directly
sqlite3 ~/.config/ambient-learner/sensors_*.db

sqlite> SELECT COUNT(*) FROM readings;
# Should increase over time

sqlite> SELECT timestamp, target FROM calibrations ORDER BY timestamp DESC LIMIT 5;
# Should show recent calibrations

sqlite> .schema
# Should see new indexes
```

### **Test QT GUI:**
```bash
# Build with QT
cargo build --features qt

# Run
cargo run --features qt

# Should show QML window
```

---

## 🎯 **Expected Improvements**

### **Sensors:**
- Active sensors: 2 → 6-10
- Better predictions
- More robust model
- Higher confidence

### **Database:**
- Save frequency: 1 hour → 5 minutes
- No more lost calibrations
- Faster history queries
- Fixed history chart

### **QT GUI:**
- Beautiful native UI
- Touch-friendly
- Better performance
- Mobile-ready

---

## 📊 **Summary**

**Total Changes:**
- Sensor files: 5 changes
- Database files: 6 changes  
- QT files: 3 new files + changes

**Estimated Time:**
- Sensors: 30 minutes
- Database: 1 hour
- QT GUI: 2-3 hours

**Total: ~4 hours for all improvements**

---

**Ready to implement - let me know which part you want to start with!** 🚀
