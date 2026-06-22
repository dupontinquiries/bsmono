# Sensor Dropout Issue & Fix - Keep More Sensors Active

## 🔍 **Problem Analysis**

### **Current Behavior:**
Only **2 sensors active out of 53** - way too aggressive!

### **Root Causes:**

**1. Dropout Score Formula Too Aggressive**
**Location:** `main.rs:626`
```rust
stat.dropout_score = 1.0 / (1.0 + stat.variance);
```

**Problem:**
- High variance → Low dropout_score → Sensor ignored
- Many sensors naturally have high variance (CPU cores, etc.)
- Formula: `variance = 10 → dropout_score = 0.09` (dropped!)
- Threshold: `dropout_score > 0.1` to be "active"

**Example:**
```
Sensor with variance 10°C → dropout_score = 1 / 11 = 0.09 → DROPPED
Sensor with variance 5°C  → dropout_score = 1 / 6  = 0.16 → KEPT
Sensor with variance 1°C  → dropout_score = 1 / 2  = 0.50 → KEPT
```

Most thermal sensors have variance > 10°C → Almost all dropped!

---

**2. Prune Weak Sensors Function Too Harsh**
**Location:** `main.rs:1246-1253`
```rust
fn prune_weak_sensors(&mut self) {
    if self.calibration_count > 5 {
        for stat in &mut self.stats {
            if stat.weight.abs() < 0.01 {
                stat.dropout_score *= 0.5;  // HALVES dropout score!
            }
        }
    }
}
```

**Problem:**
- After 5 calibrations, starts halving dropout scores
- `dropout_score *= 0.5` each time
- Sensors go: 0.16 → 0.08 → 0.04 → 0.02 (dead in 3 calls)
- This function is called but never referenced in calibrate!

---

**3. Confidence Calculation Uses Wrong Threshold**
**Location:** `main.rs:1313`
```rust
let active_sensors = self.stats.iter().filter(|s| s.dropout_score > 0.1).count();
```

**Problem:**
- Hardcoded `0.1` threshold
- Should use constant `MIN_DROPOUT_SCORE`
- But 0.1 is already too high!

---

## ✅ **Solution: Top 10% Rule + Gentler Dropout**

### **Strategy:**

1. **Keep top 10% of sensors by weight** (always keep at least 5 sensors minimum)
2. **Gentler dropout score calculation** (less penalty for variance)
3. **Adaptive threshold** (based on sensor population, not fixed 0.1)
4. **Remove prune_weak_sensors** or make it much gentler

---

## 🔧 **Implementation**

### **Fix 1: Better Dropout Score Formula**

**OLD (Too Harsh):**
```rust
stat.dropout_score = 1.0 / (1.0 + stat.variance);
```

**NEW (Gentler):**
```rust
// Square root dampens variance impact
// Variance 10 → sqrt(10) = 3.16 → dropout = 0.24 (KEPT!)
// Variance 5  → sqrt(5)  = 2.24 → dropout = 0.31 (KEPT!)
// Variance 1  → sqrt(1)  = 1.00 → dropout = 0.50 (KEPT!)
stat.dropout_score = 1.0 / (1.0 + stat.variance.sqrt());
```

Or even gentler:
```rust
// Log dampens even more
// Variance 10 → ln(11) = 2.4 → dropout = 0.29 (KEPT!)
// Variance 100 → ln(101) = 4.6 → dropout = 0.18 (KEPT!)
stat.dropout_score = 1.0 / (1.0 + (1.0 + stat.variance).ln());
```

---

### **Fix 2: Top 10% Rule in Calibration**

Add this after weight updates in `calibrate_linear`:

```rust
fn calibrate_linear(&mut self, readings: &[f32], target: f32) {
    let prediction = self.predict(readings);
    let error = target - prediction;

    self.bias += LEARNING_RATE * error;

    // Update weights
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
    
    // NEW: Apply top 10% rule (but keep at least 5 sensors)
    self.apply_top_k_rule();
}

/// Keep top K sensors by absolute weight, boost their dropout scores
fn apply_top_k_rule(&mut self) {
    let num_sensors = self.stats.len();
    let min_keep = 5; // Always keep at least 5 sensors
    let top_k = (num_sensors / 10).max(min_keep); // Top 10%, minimum 5
    
    // Create list of (index, abs_weight)
    let mut indexed_weights: Vec<(usize, f32)> = self.stats
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.weight.abs()))
        .collect();
    
    // Sort by weight descending
    indexed_weights.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    
    // Boost top-k sensors
    for &(idx, _) in indexed_weights.iter().take(top_k) {
        // Boost dropout score for top sensors (but don't exceed 1.0)
        self.stats[idx].dropout_score = (self.stats[idx].dropout_score * 1.2).min(1.0);
    }
    
    // Gently reduce scores for bottom sensors (only if below threshold)
    for &(idx, _) in indexed_weights.iter().skip(top_k) {
        if self.stats[idx].dropout_score < 0.2 {
            // Only reduce if already low
            self.stats[idx].dropout_score *= 0.95; // Very gentle reduction
        }
    }
    
    // Debug: Count active sensors
    if self.calibration_count % 5 == 0 {
        let active = self.stats.iter().filter(|s| s.dropout_score > 0.1).count();
        println!("📊 Active sensors: {} / {} (top {} rule)", active, num_sensors, top_k);
    }
}
```

---

### **Fix 3: Update Constants**

**Location:** `main.rs:267`

**OLD:**
```rust
pub const MIN_DROPOUT_SCORE: f32 = 0.1;
```

**NEW:**
```rust
// Lower threshold = keep more sensors
pub const MIN_DROPOUT_SCORE: f32 = 0.05;  // Was 0.1
```

Or make it adaptive:
```rust
impl Model {
    fn get_min_dropout_threshold(&self) -> f32 {
        let num_sensors = self.stats.len();
        // More sensors = can be more selective
        // Fewer sensors = keep more active
        if num_sensors > 50 {
            0.1
        } else if num_sensors > 20 {
            0.05
        } else {
            0.02  // Very permissive for few sensors
        }
    }
}
```

---

### **Fix 4: Remove/Fix Harsh Pruning**

**Option A: Remove entirely**
```rust
// Delete prune_weak_sensors() function
// Remove any calls to it
```

**Option B: Make much gentler**
```rust
fn prune_weak_sensors(&mut self) {
    // Only prune after many calibrations
    if self.calibration_count > 20 {
        for stat in &mut self.stats {
            // Only penalize extremely weak sensors
            if stat.weight.abs() < 0.001 {  // Changed from 0.01
                stat.dropout_score *= 0.9;  // Changed from 0.5 (gentler)
            }
        }
    }
}
```

---

### **Fix 5: Update Confidence Calculation**

**OLD:**
```rust
let active_sensors = self.stats.iter().filter(|s| s.dropout_score > 0.1).count();
```

**NEW:**
```rust
let threshold = self.get_min_dropout_threshold();
let active_sensors = self.stats.iter().filter(|s| s.dropout_score > threshold).count();
```

---

## 📊 **Expected Results**

### **Before (Current):**
```
Total sensors: 53
Active sensors: 2 (3.8%)
Dropout scores: mostly < 0.1
Confidence: Low (not enough sensors)
```

### **After (With Fixes):**
```
Total sensors: 53
Active sensors: 6-10 (11-19%)  ← Top 10% rule
Dropout scores: top sensors ~0.5-1.0, others ~0.2-0.4
Confidence: Higher (more sensor diversity)
```

---

## 🎯 **Recommended Changes Priority**

### **High Priority (Do First):**

1. ✅ **Fix dropout score formula** (gentler)
   ```rust
   stat.dropout_score = 1.0 / (1.0 + stat.variance.sqrt());
   ```

2. ✅ **Lower MIN_DROPOUT_SCORE** constant
   ```rust
   pub const MIN_DROPOUT_SCORE: f32 = 0.05;
   ```

3. ✅ **Add top 10% rule** in calibrate_linear
   ```rust
   self.apply_top_k_rule();
   ```

### **Medium Priority:**

4. ⬜ Remove or fix `prune_weak_sensors()`
5. ⬜ Add adaptive threshold based on sensor count
6. ⬜ Add debug logging to track active sensor count

### **Low Priority:**

7. ⬜ Add visualization of dropout scores in UI
8. ⬜ Add "force include sensor" override for debugging
9. ⬜ Export sensor usage stats to CSV

---

## 📝 **Code Changes Summary**

### **File: `src/main.rs`**

**Line 267: Update constant**
```rust
// OLD:
pub const MIN_DROPOUT_SCORE: f32 = 0.1;

// NEW:
pub const MIN_DROPOUT_SCORE: f32 = 0.05;
```

**Line 626: Gentler dropout formula**
```rust
// OLD:
stat.dropout_score = 1.0 / (1.0 + stat.variance);

// NEW:
stat.dropout_score = 1.0 / (1.0 + stat.variance.sqrt());
```

**After line 1243: Add top-k rule call**
```rust
fn calibrate_linear(&mut self, readings: &[f32], target: f32) {
    // ... existing code ...
    
    // NEW: Apply top 10% rule
    if self.calibration_count >= 5 {
        self.apply_top_k_rule();
    }
}
```

**After line 1259: Add new function**
```rust
/// Keep top K sensors by absolute weight, boost their dropout scores
fn apply_top_k_rule(&mut self) {
    let num_sensors = self.stats.len();
    let min_keep = 5;
    let top_k = (num_sensors / 10).max(min_keep);
    
    let mut indexed_weights: Vec<(usize, f32)> = self.stats
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.weight.abs()))
        .collect();
    
    indexed_weights.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    
    for &(idx, _) in indexed_weights.iter().take(top_k) {
        self.stats[idx].dropout_score = (self.stats[idx].dropout_score * 1.2).min(1.0);
    }
    
    for &(idx, _) in indexed_weights.iter().skip(top_k) {
        if self.stats[idx].dropout_score < 0.2 {
            self.stats[idx].dropout_score *= 0.95;
        }
    }
    
    if self.calibration_count % 5 == 0 {
        let active = self.stats.iter().filter(|s| s.dropout_score > MIN_DROPOUT_SCORE).count();
        println!("📊 Active sensors: {} / {} (keeping top {})", active, num_sensors, top_k);
    }
}
```

**Line 1313: Use constant**
```rust
// OLD:
let active_sensors = self.stats.iter().filter(|s| s.dropout_score > 0.1).count();

// NEW:
let active_sensors = self.stats.iter().filter(|s| s.dropout_score > MIN_DROPOUT_SCORE).count();
```

**Lines 1246-1259: Remove or comment out**
```rust
// REMOVE THIS ENTIRE FUNCTION - Too harsh
/*
fn prune_weak_sensors(&mut self) {
    // ... old code ...
}
*/
```

---

## 🧪 **Testing**

After applying fixes, check:

```bash
# Run and observe console output
./target/release/ambient-learner

# Look for:
📊 Active sensors: X / 53 (keeping top Y)

# Should see:
- Active sensors: 6-10 (instead of 2)
- Top sensors: 5-6 (10% of 53)
- Confidence should improve
```

---

## 🎨 **UI Visualization Idea**

Add sensor activity indicator to Model page:

```
Active Sensors: ▰▰▰▰▰▰▱▱▱▱ (6/53)
Top Contributors:
  ├─ Sensor 23 (CPU Package): 45%
  ├─ Sensor 12 (Ambient):     28%  
  └─ Sensor 7  (Core 3):      12%
```

---

## 📈 **Expected Impact**

**Before:**
- 2 active sensors
- Model relies heavily on just 1-2 readings
- Poor generalization
- Low confidence

**After:**
- 6-10 active sensors (top 10%)
- More robust predictions
- Better handles sensor variation
- Higher confidence
- Still efficient (not using all 53)

**Best of both worlds:** Sensor selection (efficiency) + enough sensors (robustness)!
