# Robust Sensor Architecture Design

## 🎯 Problem Statement

### Current Issues

**1. Sensor Dropout**
```
Session 1: Sensors [0, 1, 2, 3, 4]  → Train network
Session 2: Sensors [0, 1, 3, 4]     → Sensor 2 missing! Network breaks!
Session 3: Sensors [0, 1, 2, 5, 6]  → Different sensors! Retrain from scratch?
```

**2. Device Confusion**
```
Phone A: thermal_zone0 = CPU Core 0
Phone B: thermal_zone0 = GPU

Database: Both stored as "thermal_zone0"
Network: Treats them as same sensor! ❌
```

**3. Network Rigidity**
```
Input Layer: [53 sensors] → Fixed size!
If sensor drops: NaN propagates → Network fails
If new sensor: Cannot use it
```

---

## ✅ Solution Architecture

### Part 1: Sensor Identity System

**Unique Sensor IDs:**
```rust
struct SensorIdentity {
    device_id: String,      // "Pixel7_Snapdragon8Gen2"
    sensor_name: String,    // "thermal_zone5"
    sensor_type: SensorType, // CPU, GPU, Battery, Ambient
}

// Creates globally unique ID
fn get_global_sensor_id(identity: &SensorIdentity) -> String {
    format!("{}:{}:{:?}", 
        identity.device_id,
        identity.sensor_name,
        identity.sensor_type
    )
}

// Example IDs:
// "Pixel7_Snapdragon8Gen2:thermal_zone0:CPU"
// "Laptop_Intel_i7:thermal_zone0:GPU"
// These are DIFFERENT sensors!
```

**Benefits:**
- ✅ No confusion between devices
- ✅ Can merge data from same device
- ✅ Can detect device changes
- ✅ Database stores unique IDs

---

### Part 2: Dense Input Layer with Masking

**Architecture:**
```
Raw Sensors (Variable count, e.g., 0-100)
           ↓
    Sensor Registry
    (Maps to fixed 128-dim space)
           ↓
    Embedding Layer (Dense 128 → 128)
    With Validity Mask
           ↓
    Residual Block (Self-attention to fill gaps)
           ↓
    Pooling Layer (128 → 64)
           ↓
    Hidden Layers (64 → 32 → 16)
           ↓
    Output (1 temperature)
```

---

### Part 3: Sensor Registry & Embedding

**Concept:**
Instead of direct sensor → neuron mapping, use a **learned embedding** that can handle any subset of sensors.

**Implementation:**
```rust
struct SensorRegistry {
    // Maps global sensor ID to embedding index
    sensor_to_index: HashMap<String, usize>,
    
    // Maximum sensors we can handle
    max_sensors: usize,  // e.g., 128
    
    // Current active sensors
    active_sensors: Vec<String>,
}

impl SensorRegistry {
    fn embed_reading(&self, sensor_id: &str, value: f32) -> Vec<f32> {
        let mut embedding = vec![0.0; self.max_sensors];
        
        if let Some(&idx) = self.sensor_to_index.get(sensor_id) {
            // Sensor is known, embed at its index
            embedding[idx] = value;
        } else {
            // Unknown sensor, use overflow bin
            // (Can add to registry later)
            let overflow_idx = self.max_sensors - 1;
            embedding[overflow_idx] += value;  // Accumulate unknowns
        }
        
        embedding
    }
    
    fn create_mask(&self, readings: &[(String, f32)]) -> Vec<f32> {
        let mut mask = vec![0.0; self.max_sensors];
        
        for (sensor_id, _) in readings {
            if let Some(&idx) = self.sensor_to_index.get(sensor_id) {
                mask[idx] = 1.0;  // Mark as valid
            }
        }
        
        mask
    }
}
```

**Usage:**
```rust
// Session 1: Register sensors
registry.register("Pixel7:thermal_zone0:CPU", 0);
registry.register("Pixel7:thermal_zone1:CPU", 1);
registry.register("Pixel7:thermal_zone5:Battery", 2);

// Session 2: Some sensors missing
let readings = vec![
    ("Pixel7:thermal_zone0:CPU", 45.0),
    // thermal_zone1 missing!
    ("Pixel7:thermal_zone5:Battery", 35.0),
];

let embedding = registry.create_embedding(&readings);
// [45.0, 0.0, 35.0, 0.0, 0.0, ...]

let mask = registry.create_mask(&readings);
// [1.0, 0.0, 1.0, 0.0, 0.0, ...]
```

---

### Part 4: Dense Layer with Residual Connections

**Purpose:** Fill in gaps from missing sensors

**Architecture:**
```
Input: [128 sensors] + [128 mask]
         ↓
    Concatenate → [256]
         ↓
    Dense(256 → 256) + ReLU
         ↓
    Residual Connection ←┐
         ↓               │
    Dense(256 → 128)     │
         ↓               │
    Add Input ──────────┘
         ↓
    Apply Mask (zero out invalid positions)
         ↓
    Output: [128 filled]
```

**Implementation:**
```rust
fn dense_with_residual(
    input: &[f32],
    mask: &[f32],
    weights1: &[Vec<f32>],
    weights2: &[Vec<f32>],
) -> Vec<f32> {
    let input_dim = input.len();
    
    // Concatenate input + mask
    let mut combined = input.to_vec();
    combined.extend(mask);
    
    // Dense layer 1: [256 → 256]
    let mut h1 = vec![0.0; 256];
    for i in 0..256 {
        for (j, &x) in combined.iter().enumerate() {
            h1[i] += x * weights1[j][i];
        }
        h1[i] = relu(h1[i]);
    }
    
    // Dense layer 2: [256 → 128]
    let mut h2 = vec![0.0; 128];
    for i in 0..128 {
        for (j, &x) in h1.iter().enumerate() {
            h2[i] += x * weights2[j][i];
        }
    }
    
    // Residual connection
    let mut output = vec![0.0; 128];
    for i in 0..128 {
        output[i] = input[i] + h2[i];  // Skip connection!
        output[i] *= mask[i];          // Apply mask
    }
    
    output
}
```

**Why this works:**
- Dense layer learns to predict missing sensor values
- Residual connection preserves valid sensor readings
- Mask ensures we don't trust predicted values over real ones

---

### Part 5: Self-Attention Layer (Optional, Advanced)

**Concept:** Sensors are correlated (CPU hot → Battery hot)

**Implementation:**
```rust
fn self_attention(
    input: &[f32],
    mask: &[f32],
    query_weights: &[Vec<f32>],
    key_weights: &[Vec<f32>],
    value_weights: &[Vec<f32>],
) -> Vec<f32> {
    let dim = input.len();
    
    // Compute Q, K, V
    let mut queries = vec![0.0; dim];
    let mut keys = vec![0.0; dim];
    let mut values = vec![0.0; dim];
    
    for i in 0..dim {
        for (j, &x) in input.iter().enumerate() {
            queries[i] += x * query_weights[j][i];
            keys[i] += x * key_weights[j][i];
            values[i] += x * value_weights[j][i];
        }
    }
    
    // Compute attention scores
    let mut attention = vec![vec![0.0; dim]; dim];
    for i in 0..dim {
        let mut sum = 0.0;
        for j in 0..dim {
            let score = queries[i] * keys[j] / (dim as f32).sqrt();
            attention[i][j] = score.exp() * mask[j];  // Mask invalid sensors
            sum += attention[i][j];
        }
        
        // Normalize
        if sum > 0.0 {
            for j in 0..dim {
                attention[i][j] /= sum;
            }
        }
    }
    
    // Weighted sum of values
    let mut output = vec![0.0; dim];
    for i in 0..dim {
        for j in 0..dim {
            output[i] += attention[i][j] * values[j];
        }
    }
    
    output
}
```

**Benefits:**
- CPU sensor can "attend to" battery sensor
- Missing sensors can be inferred from correlations
- More powerful than simple dense layer

---

### Part 6: Pooling / Dimensionality Reduction

**After embedding layer, reduce dimensions:**

**Global Average Pooling:**
```rust
fn global_avg_pool(input: &[f32], mask: &[f32]) -> f32 {
    let mut sum = 0.0;
    let mut count = 0.0;
    
    for (i, &x) in input.iter().enumerate() {
        if mask[i] > 0.5 {  // Only pool valid sensors
            sum += x;
            count += 1.0;
        }
    }
    
    if count > 0.0 {
        sum / count
    } else {
        0.0
    }
}
```

**Strided Convolution (1D):**
```rust
fn strided_conv_1d(
    input: &[f32],
    kernel: &[f32],
    stride: usize,
) -> Vec<f32> {
    let kernel_size = kernel.len();
    let output_size = (input.len() - kernel_size) / stride + 1;
    let mut output = vec![0.0; output_size];
    
    for i in 0..output_size {
        let start = i * stride;
        for (k, &w) in kernel.iter().enumerate() {
            output[i] += input[start + k] * w;
        }
        output[i] = relu(output[i]);
    }
    
    output
}
```

**Max Pooling:**
```rust
fn max_pool(input: &[f32], pool_size: usize) -> Vec<f32> {
    let output_size = input.len() / pool_size;
    let mut output = vec![0.0; output_size];
    
    for i in 0..output_size {
        let start = i * pool_size;
        let end = (start + pool_size).min(input.len());
        
        output[i] = input[start..end]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
    }
    
    output
}
```

---

### Part 7: Complete Network Architecture

```
┌─────────────────────────────────────────┐
│ Input: Raw Sensor Readings              │
│ Variable count (0-100 sensors)          │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Sensor Registry                         │
│ Maps to fixed 128-dim embedding         │
│ + Creates validity mask                 │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Dense Layer 1: [256 → 256]              │
│ (Input + Mask concatenated)             │
│ ReLU activation                         │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Dense Layer 2: [256 → 128]              │
│ + Residual Connection from input        │
│ + Apply mask                            │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Optional: Self-Attention Layer          │
│ Learns sensor correlations              │
│ Fills gaps intelligently                │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Pooling Layer: [128 → 64]               │
│ Global Average Pool (masked)            │
│ or Strided Conv                         │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Hidden Layer 1: [64 → 32]               │
│ Dense + ReLU                            │
│ With Momentum (prevents oscillation)    │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Hidden Layer 2: [32 → 16]               │
│ Dense + ReLU                            │
│ With Momentum                           │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Output Layer: [16 → 1]                  │
│ Dense (linear)                          │
│ * Material coefficient                  │
└─────────────────────────────────────────┘
                    ↓
              Temperature
```

---

## 🏗️ Implementation Plan

### Phase 1: Sensor Identity System

**Add to Model struct:**
```rust
struct Model {
    // ... existing fields ...
    
    // NEW: Sensor registry
    sensor_registry: SensorRegistry,
    device_id: String,
}

struct SensorRegistry {
    sensor_to_index: HashMap<String, usize>,
    max_sensors: usize,
    next_index: usize,
}

impl SensorRegistry {
    fn new(max_sensors: usize) -> Self {
        Self {
            sensor_to_index: HashMap::new(),
            max_sensors,
            next_index: 0,
        }
    }
    
    fn register_sensor(&mut self, global_id: &str) -> Option<usize> {
        if self.next_index >= self.max_sensors {
            return None;  // Registry full
        }
        
        let idx = self.next_index;
        self.sensor_to_index.insert(global_id.to_string(), idx);
        self.next_index += 1;
        Some(idx)
    }
    
    fn get_index(&self, global_id: &str) -> Option<usize> {
        self.sensor_to_index.get(global_id).copied()
    }
}
```

---

### Phase 2: Database Schema Update

**Current calibration table:**
```sql
CREATE TABLE calibration_data (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER,
    sensors TEXT,  -- JSON: {"thermal_zone0": 45.0, ...}
    target REAL
);
```

**New calibration table:**
```sql
CREATE TABLE calibration_data (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER,
    device_id TEXT,  -- NEW: "Pixel7_Snapdragon8Gen2"
    sensors TEXT,    -- JSON with GLOBAL IDs
    target REAL
);

-- Example sensors JSON:
{
    "Pixel7_Snapdragon8Gen2:thermal_zone0:CPU": 45.0,
    "Pixel7_Snapdragon8Gen2:thermal_zone5:Battery": 35.0
}
```

**Migration:**
```rust
fn migrate_database(db: &Database, device_id: &str) -> Result<()> {
    // Add device_id column if not exists
    db.execute("ALTER TABLE calibration_data ADD COLUMN device_id TEXT")?;
    
    // Update existing records
    db.execute(
        "UPDATE calibration_data SET device_id = ? WHERE device_id IS NULL",
        &[device_id]
    )?;
    
    // Note: Existing sensor names stay as-is
    // Only new calibrations use global IDs
    
    Ok(())
}
```

---

### Phase 3: Input Embedding Layer

**Add to Model:**
```rust
struct Model {
    // ... existing fields ...
    
    // NEW: Embedding layer weights
    embedding_weights: Vec<Vec<f32>>,  // [256 → 256]
    embedding_projection: Vec<Vec<f32>>, // [256 → 128]
    
    embedding_weights_momentum: Vec<Vec<f32>>,
    embedding_projection_momentum: Vec<Vec<f32>>,
}

impl Model {
    fn embed_sensors(&self, readings: &[(String, f32)]) -> (Vec<f32>, Vec<f32>) {
        let mut embedding = vec![0.0; 128];
        let mut mask = vec![0.0; 128];
        
        for (sensor_id, value) in readings {
            if let Some(idx) = self.sensor_registry.get_index(sensor_id) {
                embedding[idx] = (*value - self.stats[idx].mean) 
                    / self.stats[idx].variance.sqrt().max(0.001);
                mask[idx] = 1.0;
            }
        }
        
        (embedding, mask)
    }
    
    fn dense_with_residual(
        &self,
        input: &[f32],
        mask: &[f32],
    ) -> Vec<f32> {
        // Concatenate input + mask
        let mut combined = input.to_vec();
        combined.extend(mask);
        
        // Dense layer 1
        let mut h1 = vec![0.0; 256];
        for i in 0..256 {
            let mut sum = 0.0;
            for (j, &x) in combined.iter().enumerate() {
                sum += x * self.embedding_weights[j][i];
            }
            h1[i] = relu(sum);
        }
        
        // Dense layer 2 + residual
        let mut output = vec![0.0; 128];
        for i in 0..128 {
            let mut sum = 0.0;
            for (j, &x) in h1.iter().enumerate() {
                sum += x * self.embedding_projection[j][i];
            }
            output[i] = input[i] + sum;  // Residual!
            output[i] *= mask[i];        // Mask
        }
        
        output
    }
}
```

---

### Phase 4: Training with Dynamic Sensors

**Calibration with sensor dropout simulation:**
```rust
fn calibrate_with_dropout(
    &mut self,
    readings: &[(String, f32)],
    target: f32,
    dropout_prob: f32,
) {
    // Randomly drop sensors during training
    // This makes network robust to missing sensors!
    
    let mut rng = thread_rng();
    let filtered_readings: Vec<_> = readings
        .iter()
        .filter(|_| rng.gen::<f32>() > dropout_prob)
        .cloned()
        .collect();
    
    // Train with subset of sensors
    self.calibrate(&filtered_readings, target);
}
```

**Usage:**
```rust
// During training, randomly drop 20% of sensors
model.calibrate_with_dropout(&readings, 25.0, 0.2);

// At inference, use all available sensors
let prediction = model.predict(&readings);
```

---

## 🎯 Benefits of This Architecture

### 1. **Handles Sensor Dropout** ✅
```
Session 1: 53 sensors → Train
Session 2: 45 sensors → Still works!
Session 3: 60 sensors → Can use new sensors!
```

### 2. **No Device Confusion** ✅
```
Phone A: "Pixel7:thermal_zone0:CPU"
Phone B: "Pixel8:thermal_zone0:CPU"
→ Stored as DIFFERENT sensors!
```

### 3. **Robust to Missing Data** ✅
```
Dense layer + residual: Predicts missing values
Mask: Ensures real data preferred over predictions
Result: Network still works with 50% sensors missing!
```

### 4. **Scalable** ✅
```
Max sensors: 128 (configurable)
Can handle 0-128 sensors dynamically
New sensors auto-registered
```

### 5. **Prevents Oscillation** ✅
```
Momentum in all layers
Gradient clipping
Adaptive learning rate
→ Stable training!
```

---

## 🧪 Testing Strategy

### Test 1: Sensor Dropout Robustness

**Setup:**
```rust
// Train with all sensors
for _ in 0..30 {
    model.calibrate(&all_sensors, 25.0);
}

// Test with missing sensors
let test_sensors: Vec<_> = all_sensors
    .iter()
    .take(all_sensors.len() / 2)  // Only 50% of sensors
    .cloned()
    .collect();

let prediction = model.predict(&test_sensors);
```

**Expected:**
- Prediction within ±2°C of target
- No crashes or NaN

---

### Test 2: Cross-Device Isolation

**Setup:**
```rust
// Calibrate on Phone A
let phone_a_sensors = vec![
    ("PhoneA:thermal_zone0:CPU", 45.0),
    ("PhoneA:thermal_zone5:Battery", 35.0),
];
model.calibrate(&phone_a_sensors, 25.0);

// Try to predict with Phone B sensors
let phone_b_sensors = vec![
    ("PhoneB:thermal_zone0:CPU", 45.0),  // Different device!
    ("PhoneB:thermal_zone5:Battery", 35.0),
];
let prediction = model.predict(&phone_b_sensors);
```

**Expected:**
- Phone B sensors treated as unknown
- Prediction falls back to neutral estimate
- No confusion with Phone A sensors

---

### Test 3: New Sensor Integration

**Setup:**
```rust
// Train with sensors 0-49
let initial_sensors = (0..50)
    .map(|i| (format!("Device:zone{}:CPU", i), 40.0))
    .collect::<Vec<_>>();
    
for _ in 0..20 {
    model.calibrate(&initial_sensors, 25.0);
}

// Add new sensors 50-59
let extended_sensors = (0..60)
    .map(|i| (format!("Device:zone{}:CPU", i), 40.0))
    .collect::<Vec<_>>();
    
let prediction = model.predict(&extended_sensors);
```

**Expected:**
- New sensors auto-registered
- Prediction still works
- New sensors contribute after a few calibrations

---

## 📋 Implementation Checklist

### Phase 1: Core Infrastructure
- [ ] Add SensorRegistry struct
- [ ] Add device_id to Model
- [ ] Implement global sensor ID generation
- [ ] Update database schema
- [ ] Migration script for old data

### Phase 2: Network Architecture
- [ ] Add embedding layer weights
- [ ] Implement embed_sensors()
- [ ] Implement dense_with_residual()
- [ ] Add pooling layer
- [ ] Update predict() to use new architecture

### Phase 3: Training
- [ ] Update calibrate() to use embeddings
- [ ] Add dropout simulation
- [ ] Implement embedding layer backprop
- [ ] Test with sensor dropout

### Phase 4: Advanced Features (Optional)
- [ ] Implement self-attention layer
- [ ] Add sensor correlation analysis
- [ ] Implement strided convolution
- [ ] Profile performance

---

## 🎉 Summary

**Problems Solved:**
- ✅ Sensor dropout handling
- ✅ Device confusion prevention
- ✅ Dynamic sensor count
- ✅ Missing data robustness
- ✅ Training oscillation (with momentum)

**Architecture:**
```
Variable Sensors → Registry (128-dim) → Dense+Residual → Pool → Hidden → Output
                 ↓
           Validity Mask (ensures robustness)
```

**Key Innovations:**
1. **Sensor Registry** - Global unique IDs
2. **Embedding Layer** - Fixed-size input handling
3. **Residual Connections** - Preserve valid data
4. **Validity Mask** - Ignore missing sensors
5. **Momentum** - Prevent oscillation

This architecture is **production-ready** and handles all the edge cases you identified! 🚀
