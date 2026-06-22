# Advanced Sensor Architecture - Dropout Robustness

## 🎯 The Core Problem

### Current Architecture Issues

**Fixed input size:**
```
[53 sensors] → Hidden1 → Hidden2 → Output

If sensor 15 drops:
❌ Network breaks (dimension mismatch)
❌ Can't handle variable inputs
❌ Sensors are position-dependent
```

**Sensor identification:**
```
Device A: thermal_zone0 = CPU
Device B: thermal_zone0 = GPU

If we merge databases:
❌ Same ID, different hardware!
❌ Predictions will be garbage
```

---

## ✅ Solution 1: Sensor Embedding Layer

### Concept: Sensors as Tokens (Like NLP)

**Instead of fixed positions, treat sensors as semantic entities:**

```rust
struct SensorReading {
    device_id: String,        // "Phone_Pixel8_53sensors"
    sensor_id: String,        // "thermal_zone15"
    sensor_type: SensorType,  // CPU, GPU, Battery, Ambient
    temperature: f32,
    timestamp: i64,
}

// Global sensor vocabulary (like word embeddings)
struct SensorEmbedding {
    // Each unique (device_id, sensor_id) gets a learned embedding vector
    embeddings: HashMap<(String, String), Vec<f32>>,  // 16-dim embedding
}
```

**Architecture:**
```
Input: Variable number of (device, sensor, temp) tuples
   ↓
Embedding Layer: Map each sensor to 16-dim vector
   ↓
Pooling Layer: Aggregate all sensors → fixed 64-dim vector
   ↓
Dense Layers: 64 → 32 → 16 → 1 (temperature)
```

**Benefits:**
- ✅ Handles any number of sensors (1 to 100+)
- ✅ Sensors can drop in/out dynamically
- ✅ Device-aware (embeddings are per-device)
- ✅ New sensors get random initialization, learn over time

---

### Implementation

**1. Sensor Embedding:**
```rust
struct SensorEmbedding {
    embedding_dim: usize,  // e.g., 16
    embeddings: HashMap<String, Vec<f32>>,  // sensor_key → embedding
}

impl SensorEmbedding {
    fn get_or_init(&mut self, device_id: &str, sensor_id: &str) -> &Vec<f32> {
        let key = format!("{}:{}", device_id, sensor_id);
        
        self.embeddings.entry(key).or_insert_with(|| {
            // Xavier initialization
            let scale = (2.0 / self.embedding_dim as f32).sqrt();
            (0..self.embedding_dim)
                .map(|_| (random() - 0.5) * scale)
                .collect()
        })
    }
    
    fn embed_sensor(&mut self, reading: &SensorReading) -> Vec<f32> {
        let embedding = self.get_or_init(&reading.device_id, &reading.sensor_id).clone();
        
        // Concatenate: [embedding (16 dims), temp (1 dim), type (4 dims one-hot)]
        let mut result = embedding;
        result.push(reading.temperature);
        
        // One-hot sensor type
        let type_onehot = match reading.sensor_type {
            SensorType::CPU => vec![1.0, 0.0, 0.0, 0.0],
            SensorType::GPU => vec![0.0, 1.0, 0.0, 0.0],
            SensorType::Battery => vec![0.0, 0.0, 1.0, 0.0],
            SensorType::Ambient => vec![0.0, 0.0, 0.0, 1.0],
            _ => vec![0.0, 0.0, 0.0, 0.0],
        };
        result.extend(type_onehot);
        
        result  // Total: 16 + 1 + 4 = 21 dims per sensor
    }
}
```

**2. Pooling Layer (Aggregate Variable Sensors):**
```rust
fn pool_sensors(embedded_sensors: Vec<Vec<f32>>) -> Vec<f32> {
    if embedded_sensors.is_empty() {
        return vec![0.0; 21];  // Return zeros if no sensors
    }
    
    let num_sensors = embedded_sensors.len();
    let dim = embedded_sensors[0].len();
    
    // Mean pooling
    let mut pooled = vec![0.0; dim];
    for sensor_vec in &embedded_sensors {
        for (i, &val) in sensor_vec.iter().enumerate() {
            pooled[i] += val / num_sensors as f32;
        }
    }
    
    // Max pooling (captures extreme temperatures)
    let mut max_pooled = vec![f32::MIN; dim];
    for sensor_vec in &embedded_sensors {
        for (i, &val) in sensor_vec.iter().enumerate() {
            max_pooled[i] = max_pooled[i].max(val);
        }
    }
    
    // Concatenate mean and max: [21 dims mean, 21 dims max] = 42 dims
    pooled.extend(max_pooled);
    
    // Add sensor count feature (normalized)
    pooled.push((num_sensors as f32 / 100.0).min(1.0));  // 0-1 range
    
    pooled  // Total: 42 + 1 = 43 dims (fixed!)
}
```

**3. Dense Network (Fixed Input Now):**
```rust
struct EmbeddingModel {
    sensor_embedding: SensorEmbedding,
    
    // Dense layers (input is FIXED 43 dims from pooling)
    dense1_weights: Vec<Vec<f32>>,  // [43 x 32]
    dense1_bias: Vec<f32>,          // [32]
    
    dense2_weights: Vec<Vec<f32>>,  // [32 x 16]
    dense2_bias: Vec<f32>,          // [16]
    
    output_weights: Vec<f32>,       // [16]
    output_bias: f32,
    
    // Momentum buffers
    embedding_momentum: HashMap<String, Vec<f32>>,
    dense1_momentum: Vec<Vec<f32>>,
    dense2_momentum: Vec<Vec<f32>>,
    output_momentum: Vec<f32>,
}

impl EmbeddingModel {
    fn predict(&mut self, readings: &[SensorReading]) -> f32 {
        // 1. Embed each sensor
        let mut embedded: Vec<Vec<f32>> = Vec::new();
        for reading in readings {
            embedded.push(self.sensor_embedding.embed_sensor(reading));
        }
        
        // 2. Pool to fixed size
        let pooled = pool_sensors(embedded);  // 43 dims
        
        // 3. Dense layer 1 (ReLU)
        let mut h1 = vec![0.0; 32];
        for i in 0..32 {
            let mut sum = self.dense1_bias[i];
            for (j, &x) in pooled.iter().enumerate() {
                sum += x * self.dense1_weights[j][i];
            }
            h1[i] = relu(sum);
        }
        
        // 4. Dense layer 2 (ReLU)
        let mut h2 = vec![0.0; 16];
        for i in 0..16 {
            let mut sum = self.dense2_bias[i];
            for (j, &x) in h1.iter().enumerate() {
                sum += x * self.dense2_weights[j][i];
            }
            h2[i] = relu(sum);
        }
        
        // 5. Output
        let mut output = self.output_bias;
        for (i, &x) in h2.iter().enumerate() {
            output += x * self.output_weights[i];
        }
        
        output
    }
}
```

---

## ✅ Solution 2: Attention-Based Sensor Fusion

### Even Better: Let Network Learn Which Sensors Matter

**Attention mechanism:**
```rust
fn sensor_attention(embedded_sensors: Vec<Vec<f32>>, query: Vec<f32>) -> Vec<f32> {
    // Compute attention scores
    let mut scores = Vec::new();
    for sensor in &embedded_sensors {
        let score = dot_product(sensor, &query);
        scores.push(score);
    }
    
    // Softmax
    let max_score = scores.iter().cloned().fold(f32::MIN, f32::max);
    let exp_scores: Vec<f32> = scores.iter()
        .map(|&s| (s - max_score).exp())
        .collect();
    let sum_exp: f32 = exp_scores.iter().sum();
    let attention: Vec<f32> = exp_scores.iter()
        .map(|&e| e / sum_exp)
        .collect();
    
    // Weighted sum
    let mut output = vec![0.0; embedded_sensors[0].len()];
    for (i, sensor) in embedded_sensors.iter().enumerate() {
        for (j, &val) in sensor.iter().enumerate() {
            output[j] += attention[i] * val;
        }
    }
    
    output
}
```

**Benefits:**
- Network learns which sensors are most important
- Automatically ignores noisy sensors
- Adapts to sensor dropout

---

## ✅ Solution 3: Device-Aware Database Schema

### Database Changes

**Old schema:**
```sql
CREATE TABLE calibration_data (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER,
    sensors TEXT,  -- Just sensor IDs
    target_temp REAL
);
```

**New schema:**
```sql
CREATE TABLE devices (
    device_id TEXT PRIMARY KEY,  -- "Phone_Pixel8_53sensors"
    device_name TEXT,
    num_sensors INTEGER,
    created_at INTEGER
);

CREATE TABLE sensor_metadata (
    device_id TEXT,
    sensor_id TEXT,              -- "thermal_zone15"
    sensor_type TEXT,            -- "CPU", "GPU", etc.
    sensor_label TEXT,           -- Human-readable name
    PRIMARY KEY (device_id, sensor_id),
    FOREIGN KEY (device_id) REFERENCES devices(device_id)
);

CREATE TABLE calibration_data (
    id INTEGER PRIMARY KEY,
    device_id TEXT,
    timestamp INTEGER,
    target_temp REAL,
    FOREIGN KEY (device_id) REFERENCES devices(device_id)
);

CREATE TABLE sensor_readings (
    calibration_id INTEGER,
    device_id TEXT,
    sensor_id TEXT,
    temperature REAL,
    FOREIGN KEY (calibration_id) REFERENCES calibration_data(id),
    FOREIGN KEY (device_id, sensor_id) REFERENCES sensor_metadata(device_id, sensor_id)
);
```

**Benefits:**
- ✅ Clear device separation
- ✅ Can't mix sensors from different devices
- ✅ Supports transfer learning (similar devices)
- ✅ Can track sensor health per device

---

### Device ID Generation

```rust
fn get_device_id() -> String {
    use std::fs;
    
    // CPU model
    let cpu_model = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "Unknown".to_string());
    
    // Clean up CPU model name
    let cpu_clean = cpu_model
        .replace(" ", "_")
        .replace("(R)", "")
        .replace("(TM)", "")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>();
    
    // Count sensors
    let sensor_count = ThermalPoller::new().sensor_names().len();
    
    // Device type (try to detect phone vs laptop)
    let device_type = if Path::new("/system/build.prop").exists() {
        "Phone"
    } else if Path::new("/proc/acpi/battery").exists() {
        "Laptop"
    } else {
        "Desktop"
    };
    
    // Combine: Type_CPU_NumSensors
    format!("{}_{}_{}sensors", device_type, cpu_clean, sensor_count)
}
```

**Example device IDs:**
```
Phone_Snapdragon_8_Gen2_53sensors
Laptop_Intel_Core_i7-1165G7_5sensors
Desktop_AMD_Ryzen_9_5950X_16sensors
```

---

## ✅ Solution 4: Input Loader with Sensor Masking

### Handle Missing Sensors Gracefully

```rust
struct SensorInputLoader {
    device_id: String,
    expected_sensors: Vec<String>,  // Sensors this device should have
    sensor_indices: HashMap<String, usize>,  // sensor_id → index
}

impl SensorInputLoader {
    fn load_readings(&self, poller: &mut ThermalPoller) -> Vec<Option<f32>> {
        let raw_readings = poller.poll().unwrap_or_default();
        let mut indexed_readings = vec![None; self.expected_sensors.len()];
        
        for (sensor_id, temp) in raw_readings.iter().zip(poller.sensor_names()) {
            if let Some(&idx) = self.sensor_indices.get(sensor_id) {
                indexed_readings[idx] = Some(*sensor_id);
            }
        }
        
        indexed_readings
    }
    
    fn create_mask(&self, readings: &[Option<f32>]) -> Vec<f32> {
        readings.iter()
            .map(|opt| if opt.is_some() { 1.0 } else { 0.0 })
            .collect()
    }
    
    fn fill_missing(&self, readings: &[Option<f32>]) -> Vec<f32> {
        let mut filled = Vec::new();
        
        for opt in readings {
            filled.push(match opt {
                Some(temp) => *temp,
                None => {
                    // Use device average or interpolation
                    35.0  // Reasonable default
                }
            });
        }
        
        filled
    }
}
```

**Usage in model:**
```rust
fn predict_with_masking(&mut self, readings: &[Option<f32>]) -> f32 {
    let mask = self.loader.create_mask(readings);
    let filled = self.loader.fill_missing(readings);
    
    // Embed with masking awareness
    let mut embedded = Vec::new();
    for (i, &temp) in filled.iter().enumerate() {
        if mask[i] > 0.5 {  // Sensor present
            let reading = SensorReading {
                device_id: self.device_id.clone(),
                sensor_id: format!("sensor_{}", i),
                temperature: temp,
                ...
            };
            embedded.push(self.embedding.embed_sensor(&reading));
        }
        // Skip absent sensors - they don't contribute to pooling
    }
    
    // Pool only present sensors
    let pooled = pool_sensors(embedded);
    
    // Continue with dense layers...
}
```

---

## 🏗️ Complete Architecture Proposal

### Final Design

```
┌─────────────────────────────────────────────────────────────┐
│ Input: Variable # of (device_id, sensor_id, temp) tuples   │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│ Sensor Embedding Layer                                      │
│ - Each unique (device, sensor) → 16-dim learned vector     │
│ - Concatenate: [embedding, temp, type_onehot]              │
│ - Output: N x 21 dims (N = # sensors present)              │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│ Attention/Pooling Layer                                     │
│ Option A: Mean + Max pooling                                │
│ Option B: Learned attention weights                        │
│ - Output: FIXED 43 dims (regardless of # sensors)          │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│ Dense Layer 1 (ReLU)                                        │
│ - 43 → 32 dims                                              │
│ - Captures sensor interactions                             │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│ Dense Layer 2 (ReLU)                                        │
│ - 32 → 16 dims                                              │
│ - Learns temperature patterns                              │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│ Output Layer                                                │
│ - 16 → 1 (predicted temperature)                           │
└─────────────────────────────────────────────────────────────┘
```

---

## 📊 Advantages Over Current Architecture

### Current (Position-Based)
```
❌ Fixed 53 sensors required
❌ Sensor 15 must always be same hardware
❌ Can't handle dropout
❌ Can't transfer between devices
❌ Database merging breaks predictions
```

### New (Embedding-Based)
```
✅ Handles 1 to 100+ sensors dynamically
✅ Each (device, sensor) has learned meaning
✅ Robust to sensor dropout
✅ Can transfer similar sensors across devices
✅ Database safely separates devices
✅ Attention learns important sensors
✅ Pooling creates fixed representation
```

---

## 🧪 Training Strategy

### Phase 1: Bootstrap from Linear (Calibrations 1-20)
```rust
// Use simple weighted average to train embeddings
for each calibration:
    for each sensor:
        embedding_grad = (prediction - target) * temp
        update_embedding(sensor, embedding_grad)
```

### Phase 2: End-to-End Training (Calibrations 20+)
```rust
// Backprop through entire network
1. Forward pass: embed → pool → dense → output
2. Compute loss: (output - target)²
3. Backprop through dense layers (already implemented)
4. Backprop through pooling (gradient flows to all sensors)
5. Backprop to embeddings (update per-sensor vectors)
```

### Momentum for Embeddings Too
```rust
// Same momentum strategy for embedding updates
embedding_momentum[sensor_key] = 
    0.9 * embedding_momentum[sensor_key] + gradient;

embedding[sensor_key] -= learning_rate * embedding_momentum[sensor_key];
```

---

## 🚀 Migration Path

### Step 1: Add Device ID (This Week)
```rust
// Add to Model struct
struct Model {
    device_id: String,
    ...
}

// Save with calibrations
db.insert_calibration_with_device(device_id, sensors, target);
```

### Step 2: Implement Embedding Layer (Next Week)
```rust
// Keep current architecture working
// Add embedding layer in parallel
// Test side-by-side
```

### Step 3: Database Migration (When Ready)
```sql
-- Migrate old data
INSERT INTO devices (device_id, device_name, num_sensors)
SELECT DISTINCT 
    get_device_id() as device_id,
    'Migrated Device' as device_name,
    COUNT(DISTINCT sensor_id) as num_sensors
FROM old_calibration_data;

-- Migrate calibrations
INSERT INTO calibration_data (device_id, timestamp, target_temp)
SELECT get_device_id(), timestamp, target_temp
FROM old_calibration_data;
```

### Step 4: Switch to Embedding Model (Future)
```rust
// Feature flag to toggle architectures
if use_embedding_model {
    model = EmbeddingModel::new(device_id);
} else {
    model = Model::new(sensor_names);  // Current
}
```

---

## 💡 Additional Ideas

### 1. Sensor Health Tracking
```rust
struct SensorHealth {
    device_id: String,
    sensor_id: String,
    readings_count: usize,
    dropout_rate: f32,      // How often sensor fails to read
    noise_level: f32,       // Variance in readings
    last_seen: Instant,
}

// Mark sensors as unreliable
if sensor.dropout_rate > 0.3 {
    // Reduce attention weight
    // Or exclude from pooling
}
```

### 2. Transfer Learning Across Devices
```rust
// If new device has similar CPU
fn transfer_embeddings(from_device: &str, to_device: &str) {
    let from_sensors = get_sensors(from_device);
    let to_sensors = get_sensors(to_device);
    
    for (from_sensor, to_sensor) in from_sensors.zip(to_sensors) {
        if from_sensor.sensor_type == to_sensor.sensor_type {
            // Copy embedding as initialization
            embeddings[to_sensor.key] = embeddings[from_sensor.key].clone();
        }
    }
}
```

### 3. Strided Convolution Alternative
```rust
// Your idea: Use conv layers for downsampling
fn conv1d_pool(sensors: &[Vec<f32>]) -> Vec<f32> {
    // Treat sensors as sequence
    // Apply 1D convolution with stride
    // Reduces dimensionality while capturing patterns
    
    let kernel_size = 3;
    let stride = 2;
    
    // This creates "local receptive fields" like you mentioned
    // Neighboring sensors influence each other
}
```

---

## ✅ Summary

### Problems Solved
1. ✅ **Sensor Dropout:** Pooling handles variable inputs
2. ✅ **Device Confusion:** Device ID in database
3. ✅ **Position Dependency:** Embeddings encode sensor meaning
4. ✅ **Transfer Learning:** Similar sensors share knowledge

### Architecture Benefits
- Embedding layer: Learns sensor semantics
- Pooling layer: Aggregates to fixed size
- Attention: Learns important sensors
- Momentum: Prevents oscillation (already implemented!)

### Implementation Priority
1. **This week:** Add device_id to database
2. **Next week:** Implement embedding layer
3. **Future:** Full attention mechanism
4. **Optional:** Strided conv experiments

**Your intuition about needing adaptive front-end layers was spot-on!** The embedding + pooling architecture solves exactly that problem. 🎯
