# Adaptive Neural Network Architecture for Dynamic Sensor Topology

## 🎯 The Real Problem

### Current Architecture (Broken)
```
Sensor 0 ──┐
Sensor 1 ──┼──→ Hidden Layer 1 → Hidden Layer 2 → Output
Sensor 2 ──┘

Problem: What if Sensor 1 drops out?
```

**Issues:**
1. Network expects **exactly N sensors** in **specific order**
2. Missing sensor = wrong input position
3. New sensor = incompatible architecture
4. Different devices = different sensor IDs
5. Can't transfer models between devices

### Example Scenario
```
Phone A (53 sensors):
- thermal_zone0: CPU Core 0
- thermal_zone1: CPU Core 1
- thermal_zone2: GPU
...

Phone B (same model, 53 sensors):
- thermal_zone0: GPU  ← Different hardware!
- thermal_zone1: CPU Core 0
- thermal_zone2: CPU Core 1
...

If we share database:
❌ thermal_zone0 from Phone A mixed with thermal_zone0 from Phone B
❌ Predictions completely wrong!
```

---

## ✅ Solution: Three-Stage Architecture

### Architecture Overview
```
Stage 1: SPARSE EMBEDDING
│ Device-aware sensor lookup
│ Maps (device_id, sensor_id) → embedding vector
│ Handles missing sensors gracefully
▼

Stage 2: DENSE SYNTHESIS  
│ Creates "synthetic sensors" from available inputs
│ Fills in missing information via learned patterns
│ Output is FIXED SIZE regardless of input sensors
▼

Stage 3: PREDICTION NETWORK
│ Standard neural network
│ Always sees same input size
│ Learns temperature patterns
▼

Output: Temperature
```

---

## 🏗️ Stage 1: Sparse Embedding Layer

### Purpose
- Map each sensor to a learned embedding
- Handle sensor dropout
- Distinguish sensors by (device_id, sensor_id)

### Implementation
```rust
struct SensorEmbedding {
    /// Embedding table: (device_id, sensor_id) → vector
    embeddings: HashMap<(String, String), Vec<f32>>,
    
    /// Embedding dimension (e.g., 8)
    embedding_dim: usize,
    
    /// Default embedding for unknown sensors
    default_embedding: Vec<f32>,
}

impl SensorEmbedding {
    fn embed(&self, device_id: &str, sensor_id: &str, temperature: f32) -> Vec<f32> {
        // Get learned embedding for this specific sensor
        let embedding = self.embeddings
            .get(&(device_id.to_string(), sensor_id.to_string()))
            .unwrap_or(&self.default_embedding);
        
        // Combine embedding with temperature reading
        let mut result = embedding.clone();
        result.push(temperature);  // Add raw reading
        
        result
    }
    
    fn embed_batch(&self, device_id: &str, sensors: &[(String, f32)]) -> Vec<Vec<f32>> {
        sensors.iter()
            .map(|(sensor_id, temp)| self.embed(device_id, sensor_id, *temp))
            .collect()
    }
}
```

### Why This Works
- Each sensor has unique identity: `("phone_a", "thermal_zone0")`
- Missing sensor → use default embedding (zero vector)
- New sensor → initialize new embedding
- Different devices → different embeddings even if same sensor name

---

## 🔄 Stage 2: Dense Synthesis Layer

### Purpose
- Create fixed-size representation from variable sensors
- Fill in missing sensor readings via learned patterns
- Create "virtual sensors" that represent common patterns

### Architecture
```
Input: Variable number of sensor embeddings (each 9D)
       [emb1, emb2, emb3, ..., embN]  where N varies

Dense Layer 1 (attention pooling):
       - Compute importance weights for each sensor
       - Aggregate: weighted_sum = Σ(weight_i × emb_i)
       
Dense Layer 2 (synthesis):
       - Transform aggregated embedding
       - Output: FIXED SIZE (e.g., 32D)
       
Output: 32D vector regardless of input sensor count
```

### Implementation
```rust
struct DenseSynthesis {
    /// Attention weights: embedding_dim → 1
    attention_weights: Vec<f32>,
    
    /// Synthesis network: aggregated_dim → fixed_output_dim
    synthesis_weights: Vec<Vec<f32>>,
    synthesis_bias: Vec<f32>,
    
    /// Output dimension (fixed)
    output_dim: usize,
}

impl DenseSynthesis {
    fn forward(&self, embeddings: &[Vec<f32>]) -> Vec<f32> {
        if embeddings.is_empty() {
            // No sensors available - return zero vector
            return vec![0.0; self.output_dim];
        }
        
        // Step 1: Compute attention weights
        let mut attention_scores = Vec::new();
        for emb in embeddings {
            let score = self.compute_attention(emb);
            attention_scores.push(score);
        }
        
        // Softmax normalization
        let max_score = attention_scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exp_scores: Vec<f32> = attention_scores.iter()
            .map(|s| (s - max_score).exp())
            .collect();
        let sum_exp: f32 = exp_scores.iter().sum();
        let attention_weights: Vec<f32> = exp_scores.iter()
            .map(|e| e / sum_exp)
            .collect();
        
        // Step 2: Weighted aggregation
        let embedding_dim = embeddings[0].len();
        let mut aggregated = vec![0.0; embedding_dim];
        
        for (emb, &weight) in embeddings.iter().zip(attention_weights.iter()) {
            for (i, &val) in emb.iter().enumerate() {
                aggregated[i] += weight * val;
            }
        }
        
        // Step 3: Synthesis to fixed size
        let mut output = self.synthesis_bias.clone();
        for i in 0..self.output_dim {
            for (j, &val) in aggregated.iter().enumerate() {
                output[i] += val * self.synthesis_weights[j][i];
            }
        }
        
        // ReLU activation
        output.iter_mut().for_each(|x| *x = x.max(0.0));
        
        output
    }
    
    fn compute_attention(&self, embedding: &[f32]) -> f32 {
        embedding.iter()
            .zip(self.attention_weights.iter())
            .map(|(e, w)| e * w)
            .sum()
    }
}
```

### Key Features
- **Attention mechanism** learns which sensors are most important
- **Fixed output size** (32D) regardless of input sensors
- **Graceful degradation** when sensors drop out
- **Learns patterns** (e.g., "if CPU sensors high, battery is probably warm")

---

## 🧠 Stage 3: Prediction Network

### Purpose
- Standard neural network
- Maps 32D synthesis → temperature
- Already implemented (our current network)

### Implementation
```rust
// This is our existing hidden layers
// But now input is ALWAYS 32D from Stage 2

struct PredictionNetwork {
    hidden1_weights: Vec<Vec<f32>>,  // [32 x 16]
    hidden1_bias: Vec<f32>,
    
    hidden2_weights: Vec<Vec<f32>>,  // [16 x 8]
    hidden2_bias: Vec<f32>,
    
    output_weights: Vec<f32>,        // [8 x 1]
    bias: f32,
}

// Forward pass (already implemented)
```

### Why This Works
- Always sees same input size (32D)
- Can be trained with standard backprop
- Momentum and all optimizations apply
- No architectural changes needed

---

## 🔧 Complete Forward Pass

### End-to-End Example
```rust
struct AdaptiveModel {
    device_id: String,
    
    // Stage 1: Sensor embeddings
    embeddings: SensorEmbedding,
    
    // Stage 2: Dense synthesis
    synthesis: DenseSynthesis,
    
    // Stage 3: Prediction network
    prediction: PredictionNetwork,
}

impl AdaptiveModel {
    fn predict(&self, sensor_readings: &[(String, f32)]) -> f32 {
        // Stage 1: Embed sensors
        let embeddings = self.embeddings.embed_batch(
            &self.device_id,
            sensor_readings
        );
        
        // Stage 2: Synthesize fixed representation
        let synthesis = self.synthesis.forward(&embeddings);
        
        // Stage 3: Predict temperature
        let temperature = self.prediction.forward(&synthesis);
        
        temperature
    }
}
```

### Example Run
```
Input: [("thermal_zone0", 45.0), ("thermal_zone2", 50.0)]
       ↓ Stage 1: Embed
       [
         [0.1, -0.2, 0.3, ..., 45.0],  # thermal_zone0 embedding + temp
         [0.2, 0.1, -0.1, ..., 50.0],  # thermal_zone2 embedding + temp
       ]
       ↓ Stage 2: Attend + Synthesize
       [0.5, -0.3, 0.8, ..., 0.2]  # 32D fixed size
       ↓ Stage 3: Predict
       12.5°C
```

---

## 📊 Database Schema Changes

### Current Schema (Broken)
```sql
CREATE TABLE calibration_data (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER,
    sensors TEXT,  -- JSON: {"thermal_zone0": 45.0, ...}
    target_temp REAL
);
```

**Problem:** Can't distinguish sensors from different devices!

### New Schema (Fixed)
```sql
CREATE TABLE calibration_data (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL,           -- NEW: Device identifier
    timestamp INTEGER,
    sensors TEXT,                       -- JSON: {"thermal_zone0": 45.0, ...}
    target_temp REAL,
    
    -- Index for efficient queries
    INDEX idx_device_timestamp (device_id, timestamp)
);

-- New table: Sensor metadata
CREATE TABLE sensor_metadata (
    device_id TEXT NOT NULL,
    sensor_id TEXT NOT NULL,
    sensor_type TEXT,                  -- CPU, GPU, Battery, etc.
    first_seen INTEGER,
    last_seen INTEGER,
    reading_count INTEGER,
    
    PRIMARY KEY (device_id, sensor_id)
);
```

### Device ID Generation
```rust
fn get_device_id() -> String {
    // Use hardware fingerprint
    let cpu_model = get_cpu_model();
    let sensor_count = ThermalPoller::new().sensor_names().len();
    let machine_id = get_machine_id();  // /etc/machine-id on Linux
    
    format!("{}_{}s_{}", 
        sanitize(&cpu_model), 
        sensor_count,
        &machine_id[..8]  // First 8 chars
    )
}

// Example: "Snapdragon_8_Gen2_53s_a3f4b2c1"
```

---

## 🎓 Training Strategy

### Phase 1: Linear Baseline (Calibrations 1-20)
```rust
// Use simple weighted average (current approach)
// Just to get started, ignore embedding complexity
```

### Phase 2: Initialize Embeddings (Calibration 20)
```rust
// Initialize embeddings based on sensor types
for (sensor_id, sensor_type) in sensors {
    match sensor_type {
        SensorType::CPU => initialize_cpu_embedding(sensor_id),
        SensorType::GPU => initialize_gpu_embedding(sensor_id),
        SensorType::Battery => initialize_battery_embedding(sensor_id),
        _ => initialize_default_embedding(sensor_id),
    }
}

// Initialize synthesis layer with identity-like weights
// (passes through aggregated embeddings with minimal transformation)
```

### Phase 3: Joint Training (Calibrations 21+)
```rust
// Train all three stages simultaneously
// Backprop through: Prediction → Synthesis → Embeddings

// Gradient flow:
// Temperature error → ∂output/∂hidden2 → ∂hidden2/∂synthesis → ∂synthesis/∂embeddings
```

### Curriculum Learning
```rust
// Early calibrations: Focus on prediction layer
let lr_prediction = 0.001;
let lr_synthesis = 0.0001;   // 10x slower
let lr_embeddings = 0.00001; // 100x slower

// Later calibrations: Balance learning rates
if calibration_count > 50 {
    lr_synthesis = 0.0005;
    lr_embeddings = 0.0001;
}
```

---

## 🛡️ Handling Sensor Dropout

### Dropout During Inference
```rust
// Some sensors missing
let available_sensors = [
    ("thermal_zone0", 45.0),
    // thermal_zone1 MISSING
    ("thermal_zone2", 50.0),
];

// Stage 1: Only embed available sensors
let embeddings = embed_batch(available_sensors);  // Only 2 embeddings

// Stage 2: Synthesis handles variable count
let synthesis = dense_synthesis.forward(&embeddings);  // Still 32D!

// Stage 3: Prediction as normal
let temp = prediction.forward(&synthesis);
```

### Dropout During Training
```rust
// Randomly drop sensors during training for robustness
fn dropout_sensors(sensors: &[(String, f32)], drop_rate: f32) -> Vec<(String, f32)> {
    sensors.iter()
        .filter(|_| rand::random::<f32>() > drop_rate)
        .cloned()
        .collect()
}

// Use during training:
let sensors_with_dropout = dropout_sensors(&sensors, 0.2);  // Drop 20%
model.calibrate(&sensors_with_dropout, target);
```

**Why this helps:**
- Network learns to predict with partial information
- Attention mechanism learns redundancy
- More robust to sensor failures

---

## 🔄 Transfer Learning Across Devices

### Save Device-Specific Embeddings
```rust
// Model file structure
{
    "device_id": "Snapdragon_8_Gen2_53s_a3f4b2c1",
    "embeddings": {
        ("Snapdragon_8_Gen2_53s_a3f4b2c1", "thermal_zone0"): [0.1, -0.2, ...],
        ("Snapdragon_8_Gen2_53s_a3f4b2c1", "thermal_zone1"): [0.2, 0.1, ...],
        ...
    },
    "synthesis": { ... },  // Device-independent!
    "prediction": { ... },  // Device-independent!
}
```

### Transfer to New Device
```rust
// Load model trained on Phone A
let model_a = Model::load("phone_a_model.json");

// Initialize for Phone B
let mut model_b = Model::new("Snapdragon_8_Gen2_53s_b5c6d3e2");

// Transfer synthesis + prediction layers (device-independent)
model_b.synthesis = model_a.synthesis.clone();
model_b.prediction = model_a.prediction.clone();

// Initialize new embeddings for Phone B sensors
model_b.embeddings = SensorEmbedding::new();

// Now Phone B starts with good synthesis/prediction
// Only needs to learn sensor embeddings (much faster!)
```

---

## 📈 Expected Benefits

### 1. Robustness
- ✅ Handles sensor dropout gracefully
- ✅ Works with different sensor counts
- ✅ Adapts to new sensors
- ✅ No architectural changes needed

### 2. Transfer Learning
- ✅ Share synthesis + prediction layers
- ✅ Only learn embeddings per device
- ✅ Faster training on new devices
- ✅ Better generalization

### 3. Device Independence
- ✅ Database tracks device_id
- ✅ No sensor confusion across devices
- ✅ Can merge databases safely (with filtering)
- ✅ Clear provenance of data

---

## 🎯 Migration Plan

### Phase 1: Database Schema (This Week)
1. Add `device_id` column to calibration_data
2. Create sensor_metadata table
3. Implement get_device_id()
4. Update Database::insert_calibration() to include device_id

### Phase 2: Input Abstraction (Next Week)
1. Create SensorEmbedding struct
2. Initialize embeddings for current sensors
3. Load/save embeddings with model
4. Test with current architecture (no synthesis yet)

### Phase 3: Dense Synthesis (Week After)
1. Implement DenseSynthesis struct
2. Initialize with identity-like weights
3. Update Model::predict() to use synthesis
4. Train end-to-end with backprop

### Phase 4: Optimization (Later)
1. Add sensor dropout during training
2. Implement attention visualization
3. Add embedding similarity analysis
4. Fine-tune learning rates

---

## 🧪 Testing Plan

### Test 1: Device ID Consistency
```rust
let device_id = get_device_id();
// Restart app
let device_id_2 = get_device_id();
assert_eq!(device_id, device_id_2);
```

### Test 2: Sensor Dropout
```rust
// Train with all sensors
model.calibrate(&all_sensors, 12.0);

// Predict with 50% sensors
let half_sensors = all_sensors[..all_sensors.len()/2];
let pred = model.predict(&half_sensors);

// Should still be reasonable (within 2°C)
assert!((pred - 12.0).abs() < 2.0);
```

### Test 3: Cross-Device Protection
```rust
// Load calibrations from Phone A
let cal_a = db.get_calibrations("device_a");

// Try to use on Phone B
let model_b = Model::new("device_b");
model_b.load_calibrations(cal_a);

// Should reject or filter
// Or use embeddings to map device_a sensors → device_b sensors
```

---

## 💡 Advanced Ideas

### 1. Sensor Relationship Learning
```rust
// Learn correlations between sensors
// "If CPU hot, GPU usually hot too"
// Use this to fill in missing sensors
```

### 2. Temporal Patterns
```rust
// Learn how sensors change over time
// "CPU spikes, then GPU follows 2 seconds later"
// Use RNN/LSTM on sensor embeddings
```

### 3. Cross-Device Clustering
```rust
// Cluster devices by thermal behavior
// "All Snapdragon 8 Gen 2 phones similar"
// Share embeddings within cluster
```

### 4. Meta-Learning
```rust
// Learn to quickly adapt to new devices
// Few-shot learning: 5 calibrations → good model
// Use synthesis layer as prior
```

---

## ✅ Summary

### Problem
- Sensors drop out
- Different devices have different sensor IDs
- Current architecture assumes fixed topology

### Solution
```
Stage 1: Sparse Embedding (device_id, sensor_id) → vector
Stage 2: Dense Synthesis (variable inputs) → fixed 32D
Stage 3: Prediction Network (32D) → temperature
```

### Benefits
- ✅ Handles sensor dropout
- ✅ Device-independent synthesis + prediction
- ✅ Transfer learning across devices
- ✅ Database tracks device provenance
- ✅ Graceful degradation

### Implementation Priority
1. **High:** Add device_id to database ← Start here!
2. **High:** Implement get_device_id()
3. **Medium:** Create SensorEmbedding layer
4. **Medium:** Implement DenseSynthesis layer
5. **Low:** Optimize attention + dropout

**This architecture solves the fundamental problem: variable sensor topology!** 🎉
