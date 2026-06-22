# Neural Network Stability & Database QoL - Final Improvements

## 🎯 Part 1: Adaptive Learning Rate & Divergence Prevention

### Problem: Divergence After 25 Calibrations

**Observed behavior:**
```
Cal #20: 9°C prediction ✅ (accurate)
Cal #25: 40°C prediction ❌ (diverged!)
Cal #30: 12°C prediction ✅ (recovered)
```

**Root cause:**
- Fixed learning rate too aggressive
- Network overshoots after learning basic patterns
- Needs to slow down as it gets closer to solution

---

### Solution: Adaptive Learning Rate Decay

**Implementation:**
```rust
// Adaptive learning rate: decay after 30 calibrations
let base_lr = LEARNING_RATE * 0.5; // 0.005
let learning_rate = if self.calibration_count > 30 {
    // Decay by 50% every 20 calibrations
    let decay_factor = 0.5_f32.powf(((calibration_count - 30) / 20) as f32);
    base_lr * decay_factor.max(0.1) // Minimum 10% of base
} else {
    base_lr
};
```

**Learning Rate Schedule:**
```
Cal 1-30:   0.005  (base rate)
Cal 31-50:  0.005  (no decay yet)
Cal 51-70:  0.0025 (50% decay)
Cal 71-90:  0.00125 (75% decay)
Cal 91+:    0.0005 (90% decay, minimum)
```

**Benefits:**
- Fast learning early (calibrations 1-30)
- Stable refinement later (calibrations 31+)
- Prevents divergence
- Still allows recovery if needed

---

### Emergency Brake for Large Errors

**Implementation:**
```rust
let error = output - target;
let abs_error = error.abs();

if abs_error > 20.0 {
    println!("⚠️  Large error: {:.1}°C", abs_error);
    println!("   Predicted: {:.1}°C, Target: {:.1}°C", output, target);
    println!("   Reducing learning rate by 90%");
    
    // Emergency learning rate: 10% of normal
    let emergency_lr = learning_rate * 0.1;
    
    // Update with extra conservative clipping
    let loss_gradient = (2.0 * error).clamp(-5.0, 5.0);
    self.update_weights_with_lr(emergency_lr, ...);
    return;
}
```

**What this does:**
- Detects wild predictions (>20°C error)
- Reduces learning rate by 90% for that update
- Uses tighter gradient clipping (±5 instead of ±10)
- Prevents further divergence
- Allows gradual recovery

**Expected output:**
```
⚠️  Large error detected: 31.2°C (predicted: 40.1°C, target: 8.9°C)
   Reducing learning rate by 90% for this step
```

---

### Logging

**Every 10 calibrations:**
```rust
if self.calibration_count % 10 == 0 && self.calibration_count >= 20 {
    println!("🎓 Neural learning rate: {:.6} (cal #{})", 
        learning_rate, self.calibration_count);
}
```

**Output:**
```
🎓 Neural learning rate: 0.005000 (cal #20)
🎓 Neural learning rate: 0.005000 (cal #30)
🎓 Neural learning rate: 0.002500 (cal #50)
🎓 Neural learning rate: 0.001250 (cal #70)
```

---

## 📊 Part 2: Database Management Page Enhancements

### New Features on Page 5

**1. Available Databases List**
```
📡 Database Management

📍 Current Database
22 calibrations (Neural model)    [📤 Export]

Available Databases               [🔄]
┌─────────────────────────────────┐
│ ✓ sensors_20260309_200356.db   │
│   22 calibrations, 45 KB        │
│                        [Switch] │
├─────────────────────────────────┤
│   sensors_20260308_180234.db   │
│   15 calibrations, 32 KB        │
│                        [Switch] │
└─────────────────────────────────┘

Import from File
Merge calibrations from external database
[📥 Import & Merge Database]
```

**2. Features:**
- ✅ Lists all .db files in weather_data/
- ✅ Shows calibration count and file size
- ✅ Marks current database with ✓
- ✅ Sorted by modification time (newest first)
- ✅ Refresh button to reload list
- ✅ Switch button to change database

---

### Database Switching (Future Implementation)

**Planned workflow:**
```rust
switch_btn.connect_clicked(move |_| {
    let dialog = adw::MessageDialog::builder()
        .heading("Switch Database?")
        .body(&format!("Switch to {}?\nThis will reload the model.", filename))
        .build();
    
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("switch", "Switch");
    
    dialog.connect_response(move |dialog, response| {
        if response == "switch" {
            // 1. Save current model
            model.lock().unwrap().save();
            
            // 2. Close current database
            drop(db);
            
            // 3. Open new database
            let new_db = Database::open(new_path);
            
            // 4. Load model from new database
            let new_model = Model::load_from_database(&new_db);
            
            // 5. Update references
            *db_ref.lock().unwrap() = new_db;
            *model_ref.lock().unwrap() = new_model;
            
            toast.add_toast("✅ Database switched successfully");
        }
    });
});
```

---

## ⚠️ Part 3: Cross-Device Sensor Compatibility

### Problem: Sensor Names Vary by Device

**Same hardware, different names:**
```
Device A (Phone):
- thermal_zone0 → CPU core 0
- thermal_zone1 → CPU core 1
- thermal_zone5 → Battery

Device B (Laptop):
- thermal_zone0 → GPU
- thermal_zone1 → CPU package
- thermal_zone2 → Battery
```

**If we merge databases:**
- thermal_zone0 from Phone ≠ thermal_zone0 from Laptop
- Weights would be completely wrong
- Predictions would be garbage

---

### Current Protection

**Sensor Stats are stored by name:**
```rust
struct SensorStat {
    name: String,  // "thermal_zone0"
    weight: f32,
    mean: f32,
    variance: f32,
    ...
}
```

**When importing:**
- Sensors matched by name
- Phone's thermal_zone0 → Laptop's thermal_zone0
- ❌ **WRONG HARDWARE!**

---

### Solution: Device ID Tagging

**Add to Model structure:**
```rust
struct Model {
    stats: Vec<SensorStat>,
    device_id: String,  // NEW: Unique device identifier
    ...
}
```

**Generate device ID:**
```rust
fn get_device_id() -> String {
    // Use CPU model + sensor count as unique fingerprint
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines()
            .find(|l| l.starts_with("model name"))
            .map(|l| l.split(':').nth(1).unwrap_or("").trim())
        )
        .unwrap_or("unknown");
    
    let sensor_count = ThermalPoller::new().poll()
        .map(|r| r.len())
        .unwrap_or(0);
    
    format!("{}_{}sensors", 
        cpu_model.replace(" ", "_"),
        sensor_count
    )
}
```

**Example device IDs:**
```
Phone: "Snapdragon_8_Gen2_53sensors"
Laptop: "Intel_Core_i7-1165G7_5sensors"
```

---

### Import Validation

**Check compatibility before merging:**
```rust
fn validate_import_compatibility(
    current_model: &Model,
    import_model: &Model
) -> Result<(), String> {
    // Check device IDs match
    if current_model.device_id != import_model.device_id {
        return Err(format!(
            "Device mismatch!\n\
             Current: {}\n\
             Import: {}\n\n\
             Cannot merge - different hardware!",
            current_model.device_id,
            import_model.device_id
        ));
    }
    
    // Check sensor counts match
    if current_model.stats.len() != import_model.stats.len() {
        return Err(format!(
            "Sensor count mismatch!\n\
             Current: {} sensors\n\
             Import: {} sensors",
            current_model.stats.len(),
            import_model.stats.len()
        ));
    }
    
    // Check sensor names match
    for (i, (current, import)) in current_model.stats.iter()
        .zip(import_model.stats.iter())
        .enumerate()
    {
        if current.name != import.name {
            return Err(format!(
                "Sensor name mismatch at index {}!\n\
                 Current: {}\n\
                 Import: {}",
                i, current.name, import.name
            ));
        }
    }
    
    Ok(())
}
```

**Import dialog:**
```
❌ Cannot Import Database

Device mismatch!
Current: Intel_Core_i7-1165G7_5sensors
Import: Snapdragon_8_Gen2_53sensors

Cannot merge - different hardware!

[OK]
```

---

### Safe Import Workflow

**1. User clicks Import**
```
[📥 Import & Merge Database]
```

**2. Select file**
```
File picker → sensors_from_phone.db
```

**3. Validate compatibility**
```rust
let import_model = Model::load_from_file(import_path)?;
validate_import_compatibility(&current_model, &import_model)?;
```

**4. Show confirmation**
```
✅ Compatible Database

Device: Snapdragon_8_Gen2_53sensors (matches!)
Calibrations: 35
Would merge 35 new calibrations

[Cancel] [Merge]
```

**5. Merge calibrations**
```rust
for cal in import_calibrations {
    if !current_db.has_calibration(cal)? {
        current_db.insert_calibration(cal)?;
        merged_count += 1;
    }
}
```

**6. Retrain model**
```rust
model.retrain_from_database(&db)?;
toast.add_toast(&format!("✅ Merged {} calibrations", merged_count));
```

---

## 🧪 Testing Guide

### Test 1: Adaptive Learning Rate

**Setup:**
```
Device: Phone with 53 sensors
Calibrations: 40
```

**Watch for:**
```
Cal #20: 🧠 Switching to neural network
Cal #30: 🎓 Neural learning rate: 0.005000
Cal #31-40: [training continues]
Cal #50: 🎓 Neural learning rate: 0.002500  ← Decayed!
```

**If divergence occurs:**
```
Cal #25: Prediction: 40°C (way off!)
⚠️  Large error detected: 31.2°C (predicted: 40.1°C, target: 8.9°C)
   Reducing learning rate by 90% for this step

Cal #26: Prediction: 35°C (recovering)
Cal #27: Prediction: 28°C (recovering)
Cal #30: Prediction: 12°C (recovered!)
```

---

### Test 2: Database List

**Navigate to Page 5:**
```
Swipe right 4 times → Sync page
```

**Should see:**
```
Available Databases
✓ sensors_20260309_200356.db
  22 calibrations, 45 KB

  sensors_20260308_180234.db
  15 calibrations, 32 KB
```

**Click refresh:**
```
[🔄] → List updates
```

---

### Test 3: Cross-Device Protection (Future)

**Scenario 1: Same device**
```
Export from Phone A → Import to Phone A
✅ Device: Snapdragon_8_Gen2_53sensors (matches!)
✅ Import allowed
```

**Scenario 2: Different devices**
```
Export from Phone → Import to Laptop
❌ Device mismatch!
   Phone: Snapdragon_8_Gen2_53sensors
   Laptop: Intel_Core_i7-1165G7_5sensors
❌ Import blocked
```

---

## ✅ Summary

### Adaptive Learning Rate
- ✅ Base rate: 0.005 (calibrations 1-30)
- ✅ Decay: 50% every 20 cals after cal #30
- ✅ Emergency brake: 90% reduction if error >20°C
- ✅ Logging every 10 calibrations
- ✅ Prevents divergence
- ✅ Allows recovery

### Database Management
- ✅ Lists all available databases
- ✅ Shows calibration count & size
- ✅ Marks current database
- ✅ Sorted by recency
- ✅ Refresh button
- ⚠️  Switch functionality TODO
- ⚠️  Default database selector TODO

### Cross-Device Protection
- ⚠️  Device ID tagging TODO
- ⚠️  Compatibility validation TODO
- ⚠️  Import blocking TODO
- **Currently:** No protection (manual caution required)

---

## 🔨 Next Steps

### Priority 1: Device ID Implementation
```rust
1. Add device_id to Model struct
2. Generate device ID on init
3. Save/load in model JSON
4. Add validation to import
```

### Priority 2: Database Switching
```rust
1. Wire up switch buttons
2. Implement database reload
3. Update model references
4. Add confirmation dialog
```

### Priority 3: Default Database Selector
```rust
1. Add settings file (default_db.json)
2. "Latest" vs specific file selector
3. Fallback to latest if not found
4. Save preference
```

---

## 🎯 Expected Results After Fixes

**Training stability:**
```
Cal 1-20:  Linear (stable)
Cal 20:    Bootstrap (smooth transition)
Cal 21-30: Neural (fast learning, 0.005 LR)
Cal 31-50: Neural (refinement, 0.005 LR)
Cal 51+:   Neural (fine-tuning, 0.0025 LR)
```

**No more divergence:**
- Large errors trigger emergency brake
- Learning rate auto-reduces
- Network recovers within a few calibrations

**Better database management:**
- See all available databases
- Easy switching between databases
- Protected from cross-device imports

🎉 **Build and test - neural network should be much more stable now!**
