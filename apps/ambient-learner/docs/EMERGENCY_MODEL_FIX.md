# EMERGENCY FIX - Model Runaway Divergence

## 🚨 Critical Issue

**Your model has exploded:**
```
Cal #42: Predicted: -23.4°C (actual: 7°C) - error: 30°C
Cal #43: Predicted: -794.6°C (actual: 7°C) - error: 801°C  
Cal #44: Predicted: -2036.3°C (actual: 7°C) - error: 2043°C
Cal #50: Predicted: -6094.1°C (actual: 7°C) - error: 6101°C ❌
```

**Root cause: Sensor dropout + unstable weights**

---

## ⚠️ Immediate Actions Required

### 1. Delete Broken Model Files

```bash
cd weather_data
rm model_20260310_*.json
```

**Why:** Model weights are corrupted beyond recovery

### 2. Keep Database (It's OK)

```bash
# Database is fine - 132 calibrations stored
# Don't delete sensors_*.db files
```

---

## 🔧 Code Fixes Needed

### Fix 1: Sensor Dropout Detection

**Problem:** Model expects 53 sensors, but sometimes only gets 11

**Current code doesn't check:**
```rust
fn predict(&mut self, readings: &[f32]) -> f32 {
    // Assumes readings.len() == stats.len()
    // WRONG if sensors dropped out!
}
```

**Fix:**
```rust
fn predict(&mut self, readings: &[f32]) -> f32 {
    // Safety check
    if readings.len() != self.stats.len() {
        eprintln!("⚠️  Sensor count mismatch! Expected {}, got {}",
            self.stats.len(), readings.len());
        
        // Return last known good prediction
        return self.last_prediction;
    }
    
    // ... rest of prediction code ...
}
```

**Add to calibrate():**
```rust
fn calibrate(&mut self, readings: &[f32], target: f32) {
    // CRITICAL: Check sensor count matches
    if readings.len() != self.stats.len() {
        eprintln!("❌ Cannot calibrate: sensor mismatch! Expected {}, got {}",
            self.stats.len(), readings.len());
        eprintln!("   Skipping this calibration to prevent model corruption");
        return;  // Don't train on mismatched data!
    }
    
    // ... rest of calibration code ...
}
```

---

### Fix 2: Weight Explosion Protection

**Add to update_weights_with_lr():**

```rust
fn update_weights_with_lr(...) {
    // ... existing code ...
    
    // AFTER updating all weights, check for explosion
    let max_weight = self.hidden1_weights.iter()
        .flat_map(|row| row.iter())
        .map(|w| w.abs())
        .fold(0.0f32, f32::max);
    
    if max_weight > 100.0 {
        eprintln!("🚨 WEIGHT EXPLOSION DETECTED! Max weight: {:.1}", max_weight);
        eprintln!("   Resetting model to prevent corruption");
        
        // Re-initialize all weights
        self.initialize_neural_weights();
        
        // Reset momentum buffers
        self.hidden1_weights_momentum = vec![vec![0.0; hidden1_size]; num_sensors];
        self.hidden2_weights_momentum = vec![vec![0.0; hidden2_size]; hidden1_size];
        self.output_weights_momentum = vec![0.0; hidden2_size];
        self.bias_momentum = 0.0;
        
        eprintln!("✓ Model reset complete");
    }
}
```

---

### Fix 3: Prediction Sanity Checks

**Add after forward pass:**

```rust
let mut output = self.bias;
for (i, &x) in h2.iter().enumerate() {
    if i < self.output_weights.len() {
        output += x * self.output_weights[i];
    }
}
output *= self.material_coefficient;

// SANITY CHECK
if output.abs() > 200.0 {
    eprintln!("⚠️  Insane prediction: {:.1}°C - using fallback", output);
    
    // Fallback to simple average of sensor readings
    let avg: f32 = readings.iter().sum::<f32>() / readings.len() as f32;
    output = avg - 20.0;  // Rough correction for internal heat
}

if output.is_nan() || output.is_infinite() {
    eprintln!("⚠️  NaN/Inf prediction - using last known value");
    return self.last_prediction;
}
```

---

### Fix 4: Emergency Brake Should SKIP Training

**Current code still trains after emergency brake - WRONG!**

```rust
if abs_error > 20.0 {
    println!("⚠️  Large error detected: {:.1}°C", abs_error);
    println!("   Reducing learning rate by 90% for this step");
    
    // WRONG: Still trains with tiny LR
    self.update_weights_with_lr(emergency_lr, ...);
    return;
}
```

**Should be:**

```rust
if abs_error > 50.0 {  // Higher threshold
    println!("🚨 CRITICAL ERROR: {:.1}°C (predicted: {:.1}°C, target: {:.1}°C)", 
        abs_error, output, target);
    println!("   SKIPPING THIS CALIBRATION - model may be diverging");
    
    // DON'T TRAIN AT ALL!
    return;  // Just skip this calibration entirely
}

if abs_error > 20.0 {
    println!("⚠️  Large error: {:.1}°C - using reduced learning rate", abs_error);
    
    // OK to train with emergency LR
    self.update_weights_with_lr(emergency_lr, ...);
    return;
}
```

---

## 🔄 Model Recovery Procedure

### Option A: Fresh Start (Recommended)

```bash
# 1. Backup everything
cp -r weather_data weather_data.backup

# 2. Delete broken models
cd weather_data
rm model_*.json

# 3. Restart app
# Model will initialize fresh with 0 calibrations
# Database still has 132 calibrations for reference
```

### Option B: Reinitialize from Database

**Not recommended** - database has mixed sensor counts

---

## 📊 Why This Happened

### Sensor Count Mismatch

**From logs:**
```
Startup: 📊 Total sensors: 53
Model loaded: Active sensors: 11  ← WRONG!
Calibration: 📊 Total sensors: 53  ← Back to 53
```

**What happened:**
1. Old model was saved with 53 sensors
2. At some point, only 11 sensors were active (some failed to read)
3. Model pruned "weak" sensors (dropout_score = 0)
4. Model size became 11 sensors
5. New calibrations come in with 53 sensors
6. **Dimension mismatch** → network predicts garbage
7. Garbage predictions → huge errors → momentum carries wrong direction
8. **Runaway divergence** → model explodes

---

## 🛡️ Prevention

### 1. Lock Sensor Count

```rust
struct Model {
    expected_sensor_count: usize,  // Lock this on first calibration
    ...
}

impl Model {
    fn calibrate(&mut self, readings: &[f32], target: f32) {
        // Lock sensor count on first calibration
        if self.calibration_count == 0 {
            self.expected_sensor_count = readings.len();
        }
        
        // Reject mismatched calibrations
        if readings.len() != self.expected_sensor_count {
            eprintln!("❌ Sensor count changed! Expected {}, got {}",
                self.expected_sensor_count, readings.len());
            return;
        }
        
        // ... rest of code ...
    }
}
```

### 2. Disable Sensor Pruning for Now

```rust
fn prune_weak_sensors(&mut self) {
    // DISABLED - causes sensor count mismatch
    // Re-enable when we have proper handling
    return;
}
```

### 3. Add Model Validation on Load

```rust
fn load(path: &Path) -> Result<Self> {
    let mut model: Model = serde_json::from_str(&contents)?;
    
    // Validate model isn't corrupted
    let max_weight = model.hidden1_weights.iter()
        .flat_map(|row| row.iter())
        .map(|w| w.abs())
        .fold(0.0f32, f32::max);
    
    if max_weight > 100.0 || model.bias.abs() > 1000.0 {
        return Err("Model appears corrupted (extreme weights)".into());
    }
    
    // ... rest of load code ...
}
```

---

## 🧪 Testing After Fix

### Test 1: Fresh Model

```bash
# Delete old models
rm weather_data/model_*.json

# Start app
cargo run --release

# Add 5 calibrations at same temp (21°C)
# Should see:
# Cal #1: ~30°C (random init)
# Cal #2: ~25°C (improving)
# Cal #3: ~23°C (converging)
# Cal #4: ~21.5°C (close)
# Cal #5: ~21.2°C (good!)

# NOT: -6000°C!
```

### Test 2: Sensor Dropout Protection

```bash
# Watch logs carefully
# Should never see:
# "sensor mismatch" during calibration
# If you do, it means sensors are still dropping out
```

---

## ✅ Quick Fix Summary

**Immediate:**
1. `rm weather_data/model_*.json` - Delete broken models
2. Restart app - fresh model

**Code changes needed (in order):**
1. Add sensor count check to `predict()` and `calibrate()` - **CRITICAL**
2. Skip calibration if error > 50°C - **CRITICAL**
3. Add weight explosion detection - **HIGH**
4. Add prediction sanity checks - **HIGH**
5. Disable sensor pruning - **MEDIUM**

**The model will work fine after these fixes!** The momentum implementation is actually correct - the problem was sensor mismatch causing garbage inputs. 🔧
