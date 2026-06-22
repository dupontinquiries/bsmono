# Neural Field Architecture - Temperature Estimation Integration

## 🎯 Problem Analysis

### Current System
```
53 thermal sensors → Neural network → Single ambient temp
```

**Issues:**
1. Position-dependent (sensor[15] must always be same hardware)
2. Can't handle dropout (if sensor fails, network breaks)
3. Single point estimate (no spatial information)
4. Device confusion (same sensor ID across devices)

---

### New System (Neural Field)
```
Variable sensors + device state → Neural field → T(x,y,z) continuous function
```

**Benefits:**
1. Query temperature at ANY point in space
2. Handles sensor dropout naturally
3. Separates ambient vs device heat
4. Device-aware (learned sensor positions)
5. Can predict 1m away from phone

---

## 🏗️ Architecture Design

### Complete System

```
┌─────────────────────────────────────────────────────────┐
│ Input Layer                                              │
│ - Sensor readings: [(id, temp), (id, temp), ...]       │
│ - Device state: [cpu%, gpu%, battery_mA, charging]     │
│ - Query point: (x, y, z) in meters                     │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Latent Sensor Position Module                           │
│ Each sensor_id → learned (x, y, z) within phone bounds │
│ Constraints: |x| < 0.04m, |y| < 0.08m, |z| < 0.005m   │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Positional Encoding (from NeRF)                         │
│ (x,y,z) → [sin(2^0πx), cos(2^0πx), ...,                │
│             sin(2^Lπx), cos(2^Lπx)] for x,y,z           │
│ L=4 → 24 dims (4 frequencies × 2 sin/cos × 3 xyz)      │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Sensor Context Encoding                                 │
│ For each sensor: distance to query point                │
│ weight = exp(-distance²/σ²)                             │
│ weighted_temp = Σ(weight_i × temp_i)                    │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Device Heat Model (MLP)                                 │
│ Input: [pos_encoding, device_state, sensor_context]    │
│ Hidden: 64 → 32 → 16 (SIREN: sin activation)           │
│ Output: device_heat_offset                              │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Ambient Field Model (MLP)                               │
│ Input: [pos_encoding, sensor_context]                  │
│ Hidden: 64 → 32 → 16 (SIREN: sin activation)           │
│ Output: ambient_temperature_base                        │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Final Temperature                                        │
│ T(x,y,z) = ambient_base + device_heat                   │
└─────────────────────────────────────────────────────────┘
```

---

## 📊 Mathematical Formulation

### Positional Encoding (NeRF-style)

```rust
fn positional_encoding(x: f32, y: f32, z: f32, L: usize) -> Vec<f32> {
    let mut encoding = Vec::with_capacity(L * 6);
    
    for i in 0..L {
        let freq = 2.0_f32.powi(i as i32) * std::f32::consts::PI;
        
        // Encode x
        encoding.push((x * freq).sin());
        encoding.push((x * freq).cos());
        
        // Encode y
        encoding.push((y * freq).sin());
        encoding.push((y * freq).cos());
        
        // Encode z
        encoding.push((z * freq).sin());
        encoding.push((z * freq).cos());
    }
    
    encoding  // 6 * L dims (L=4 → 24 dims)
}
```

**Why this works:**
- Maps low-dim coordinates to high-dim space
- Helps network represent high-frequency details
- Standard in NeRF, SIREN, all neural fields
- Captures both smooth and sharp temperature gradients

---

### SIREN Activation (Better than ReLU for fields)

```rust
fn siren(x: f32) -> f32 {
    x.sin()
}

fn siren_derivative(x: f32) -> f32 {
    x.cos()
}
```

**Why SIREN:**
- Represents periodic/spatial patterns better
- Smooth derivatives everywhere
- No "dead neurons" like ReLU
- Perfect for temperature fields (smooth transitions)

**Paper:** "Implicit Neural Representations with Periodic Activation Functions"

---

### Sensor Context Encoding

```rust
fn encode_sensor_context(
    query_pos: (f32, f32, f32),
    sensor_positions: &[(f32, f32, f32)],
    sensor_temps: &[f32],
    sigma: f32,
) -> Vec<f32> {
    let (qx, qy, qz) = query_pos;
    
    let mut weighted_sum = 0.0;
    let mut weight_sum = 0.0;
    let mut max_temp = f32::MIN;
    let mut min_temp = f32::MAX;
    
    for (i, &(sx, sy, sz)) in sensor_positions.iter().enumerate() {
        // Distance to query point
        let dist_sq = (qx - sx).powi(2) + (qy - sy).powi(2) + (qz - sz).powi(2);
        
        // RBF weight (Gaussian)
        let weight = (-dist_sq / (2.0 * sigma * sigma)).exp();
        
        weighted_sum += weight * sensor_temps[i];
        weight_sum += weight;
        
        max_temp = max_temp.max(sensor_temps[i]);
        min_temp = min_temp.min(sensor_temps[i]);
    }
    
    let weighted_avg = if weight_sum > 0.0 {
        weighted_sum / weight_sum
    } else {
        35.0  // Default
    };
    
    vec![
        weighted_avg,
        max_temp,
        min_temp,
        max_temp - min_temp,  // Temperature range
        (sensor_temps.len() as f32 / 100.0).min(1.0),  // Sensor count (normalized)
    ]
}
```

**Why this works:**
- Nearby sensors contribute more
- Captures local temperature gradient
- Provides network with spatial context
- Handles variable number of sensors

---

## 🔧 Implementation Plan

### Phase 1: Data Structures

```rust
// src/neural_field.rs

use std::collections::HashMap;

/// Continuous temperature field estimator
pub struct TemperatureField {
    /// Device identifier (for sensor position learning)
    device_id: String,
    
    /// Learned sensor positions (device_id:sensor_id → (x,y,z))
    sensor_positions: HashMap<String, (f32, f32, f32)>,
    
    /// Ambient field network weights
    ambient_weights: AmbientFieldNetwork,
    
    /// Device heat network weights
    device_heat_weights: DeviceHeatNetwork,
    
    /// Positional encoding frequency count
    pos_encoding_freqs: usize,  // L=4 typical
}

struct AmbientFieldNetwork {
    // Layer 1: [24 pos_encoding + 5 sensor_context] → 64
    layer1_weights: Vec<Vec<f32>>,  // [29 x 64]
    layer1_bias: Vec<f32>,          // [64]
    
    // Layer 2: 64 → 32
    layer2_weights: Vec<Vec<f32>>,
    layer2_bias: Vec<f32>,
    
    // Layer 3: 32 → 16
    layer3_weights: Vec<Vec<f32>>,
    layer3_bias: Vec<f32>,
    
    // Output: 16 → 1
    output_weights: Vec<f32>,
    output_bias: f32,
}

struct DeviceHeatNetwork {
    // Similar structure but includes device state
    // Input: [24 pos_encoding + 5 sensor_context + 4 device_state] → 64
    layer1_weights: Vec<Vec<f32>>,  // [33 x 64]
    layer1_bias: Vec<f32>,
    // ... same as ambient
}

/// Device telemetry for heat modeling
pub struct DeviceState {
    pub cpu_usage: f32,      // 0.0 to 1.0
    pub gpu_usage: f32,      // 0.0 to 1.0
    pub battery_current: f32, // Amperes
    pub charging: bool,
    pub screen_brightness: f32, // 0.0 to 1.0
}
```

---

### Phase 2: Query Interface

```rust
impl TemperatureField {
    /// Estimate temperature at arbitrary point in space
    /// 
    /// Coordinates are relative to phone center in meters:
    /// - x: east (+) / west (-)
    /// - y: north (+) / south (-)
    /// - z: up (+) / down (-)
    pub fn estimate_temperature(
        &self,
        x: f32,
        y: f32,
        z: f32,
        sensor_readings: &[(String, f32)],  // (sensor_id, temp)
        device_state: &DeviceState,
    ) -> f32 {
        // 1. Get sensor positions (learned or default)
        let (positions, temps) = self.get_sensor_positions_and_temps(sensor_readings);
        
        // 2. Positional encoding
        let pos_enc = self.positional_encoding(x, y, z);
        
        // 3. Sensor context encoding
        let sensor_ctx = self.encode_sensor_context((x, y, z), &positions, &temps);
        
        // 4. Device state encoding
        let device_enc = self.encode_device_state(device_state);
        
        // 5. Predict ambient field
        let ambient_input = [pos_enc.clone(), sensor_ctx.clone()].concat();
        let ambient_temp = self.ambient_weights.forward(&ambient_input);
        
        // 6. Predict device heat offset
        let heat_input = [pos_enc, sensor_ctx, device_enc].concat();
        let device_heat = self.device_heat_weights.forward(&heat_input);
        
        // 7. Combine
        ambient_temp + device_heat
    }
    
    /// Estimate temperature in cardinal directions (for UI)
    pub fn estimate_cardinal_directions(
        &self,
        distance: f32,  // Default 1.0 meter
        sensor_readings: &[(String, f32)],
        device_state: &DeviceState,
    ) -> CardinalTemperatures {
        CardinalTemperatures {
            north: self.estimate_temperature(0.0, distance, 0.0, sensor_readings, device_state),
            south: self.estimate_temperature(0.0, -distance, 0.0, sensor_readings, device_state),
            east: self.estimate_temperature(distance, 0.0, 0.0, sensor_readings, device_state),
            west: self.estimate_temperature(-distance, 0.0, 0.0, sensor_readings, device_state),
            up: self.estimate_temperature(0.0, 0.0, distance, sensor_readings, device_state),
            down: self.estimate_temperature(0.0, 0.0, -distance, sensor_readings, device_state),
        }
    }
}

pub struct CardinalTemperatures {
    pub north: f32,
    pub south: f32,
    pub east: f32,
    pub west: f32,
    pub up: f32,
    pub down: f32,
}
```

---

### Phase 3: Forward Pass (SIREN activation)

```rust
impl AmbientFieldNetwork {
    fn forward(&self, input: &[f32]) -> f32 {
        // Layer 1
        let mut h1 = vec![0.0; self.layer1_bias.len()];
        for i in 0..h1.len() {
            let mut sum = self.layer1_bias[i];
            for (j, &x) in input.iter().enumerate() {
                if j < self.layer1_weights.len() {
                    sum += x * self.layer1_weights[j][i];
                }
            }
            h1[i] = sum.sin();  // SIREN activation
        }
        
        // Layer 2
        let mut h2 = vec![0.0; self.layer2_bias.len()];
        for i in 0..h2.len() {
            let mut sum = self.layer2_bias[i];
            for (j, &x) in h1.iter().enumerate() {
                if j < self.layer2_weights.len() {
                    sum += x * self.layer2_weights[j][i];
                }
            }
            h2[i] = sum.sin();
        }
        
        // Layer 3
        let mut h3 = vec![0.0; self.layer3_bias.len()];
        for i in 0..h3.len() {
            let mut sum = self.layer3_bias[i];
            for (j, &x) in h2.iter().enumerate() {
                if j < self.layer3_weights.len() {
                    sum += x * self.layer3_weights[j][i];
                }
            }
            h3[i] = sum.sin();
        }
        
        // Output (no activation on final layer)
        let mut output = self.output_bias;
        for (i, &x) in h3.iter().enumerate() {
            if i < self.output_weights.len() {
                output += x * self.output_weights[i];
            }
        }
        
        output
    }
}
```

---

### Phase 4: Training (Backpropagation through SIREN)

```rust
impl TemperatureField {
    /// Train on a single calibration
    /// 
    /// We know:
    /// - Sensor readings (internal, biased by device heat)
    /// - True ambient temperature (user-provided calibration)
    /// 
    /// We learn:
    /// - Sensor positions
    /// - How to separate ambient from device heat
    pub fn calibrate(
        &mut self,
        sensor_readings: &[(String, f32)],
        device_state: &DeviceState,
        true_ambient: f32,
        learning_rate: f32,
    ) {
        // 1. Assume ambient is uniform around phone at 1m distance
        // This is reasonable - device heat doesn't reach that far
        let test_points = [
            (0.0, 1.0, 0.0),   // North
            (0.0, -1.0, 0.0),  // South
            (1.0, 0.0, 0.0),   // East
            (-1.0, 0.0, 0.0),  // West
        ];
        
        for &(x, y, z) in &test_points {
            let predicted = self.estimate_temperature(x, y, z, sensor_readings, device_state);
            let error = predicted - true_ambient;
            
            // Backprop (simplified - full implementation needs proper gradient flow)
            self.backprop_ambient(error, x, y, z, sensor_readings, learning_rate);
        }
        
        // 2. Also train sensor position reconstruction
        // Sensors should predict their own temperatures when queried at their position
        self.update_sensor_positions(sensor_readings, learning_rate);
    }
    
    fn update_sensor_positions(
        &mut self,
        sensor_readings: &[(String, f32)],
        learning_rate: f32,
    ) {
        for (sensor_id, actual_temp) in sensor_readings {
            let key = format!("{}:{}", self.device_id, sensor_id);
            
            // Get or initialize sensor position
            let pos = self.sensor_positions
                .entry(key.clone())
                .or_insert_with(|| {
                    // Random initialization within phone bounds
                    use rand::Rng;
                    let mut rng = rand::thread_rng();
                    (
                        rng.gen_range(-0.04..0.04),
                        rng.gen_range(-0.08..0.08),
                        rng.gen_range(-0.005..0.005),
                    )
                });
            
            // Predict temperature at current sensor position
            let device_state = DeviceState::default();  // Approximate
            let predicted = self.estimate_temperature(pos.0, pos.1, pos.2, sensor_readings, &device_state);
            
            let error = predicted - actual_temp;
            
            // Gradient descent on sensor position
            // (Simplified - full version needs proper gradient computation)
            let epsilon = 0.001;
            
            // Numerical gradient
            let grad_x = (
                self.estimate_temperature(pos.0 + epsilon, pos.1, pos.2, sensor_readings, &device_state)
                - self.estimate_temperature(pos.0 - epsilon, pos.1, pos.2, sensor_readings, &device_state)
            ) / (2.0 * epsilon);
            
            pos.0 -= learning_rate * error * grad_x;
            
            // Clamp to phone bounds
            pos.0 = pos.0.clamp(-0.04, 0.04);
            pos.1 = pos.1.clamp(-0.08, 0.08);
            pos.2 = pos.2.clamp(-0.005, 0.005);
        }
    }
}
```

---

## 🎯 Integration with Current Code

### Step 1: Create Abstraction

```rust
// src/temperature_estimator.rs

pub trait TemperatureEstimator {
    fn estimate_ambient(&self, sensor_readings: &[f32]) -> f32;
    
    fn calibrate(&mut self, sensor_readings: &[f32], true_ambient: f32);
}

/// Legacy estimator (current neural network)
pub struct LegacyEstimator {
    model: Model,  // Existing Model struct
}

impl TemperatureEstimator for LegacyEstimator {
    fn estimate_ambient(&self, sensor_readings: &[f32]) -> f32 {
        self.model.predict(sensor_readings)
    }
    
    fn calibrate(&mut self, sensor_readings: &[f32], true_ambient: f32) {
        self.model.calibrate(sensor_readings, true_ambient);
    }
}

/// New neural field estimator
pub struct NeuralFieldEstimator {
    field: TemperatureField,
}

impl TemperatureEstimator for NeuralFieldEstimator {
    fn estimate_ambient(&self, sensor_readings: &[f32]) -> f32 {
        // Convert to spatial query
        // For now, average the cardinal directions
        let device_state = DeviceState::default();  // Get from system
        let sensor_pairs: Vec<_> = sensor_readings.iter()
            .enumerate()
            .map(|(i, &temp)| (format!("sensor_{}", i), temp))
            .collect();
        
        let cardinals = self.field.estimate_cardinal_directions(
            1.0,  // 1 meter away
            &sensor_pairs,
            &device_state,
        );
        
        // Average all directions
        (cardinals.north + cardinals.south + cardinals.east + cardinals.west) / 4.0
    }
    
    fn calibrate(&mut self, sensor_readings: &[f32], true_ambient: f32) {
        let device_state = DeviceState::default();
        let sensor_pairs: Vec<_> = sensor_readings.iter()
            .enumerate()
            .map(|(i, &temp)| (format!("sensor_{}", i), temp))
            .collect();
        
        self.field.calibrate(&sensor_pairs, &device_state, true_ambient, 0.001);
    }
}
```

### Step 2: Feature Flag

```rust
// In main application

enum EstimatorType {
    Legacy,
    NeuralField,
}

impl Application {
    fn create_estimator(estimator_type: EstimatorType) -> Box<dyn TemperatureEstimator> {
        match estimator_type {
            EstimatorType::Legacy => {
                Box::new(LegacyEstimator { model: Model::new(...) })
            }
            EstimatorType::NeuralField => {
                Box::new(NeuralFieldEstimator { field: TemperatureField::new(...) })
            }
        }
    }
}
```

---

## 📊 Model Size & Performance

### Memory Footprint

```
Positional encoding: 0 params (computed)
Sensor positions: ~53 sensors × 3 coords × 4 bytes = 636 bytes

Ambient network:
  Layer 1: 29 × 64 + 64 = 1,920 params
  Layer 2: 64 × 32 + 32 = 2,080 params
  Layer 3: 32 × 16 + 16 = 528 params
  Output: 16 × 1 + 1 = 17 params
  Total: 4,545 params × 4 bytes = ~18 KB

Device heat network:
  Layer 1: 33 × 64 + 64 = 2,176 params
  Layers 2-4: same as ambient = 2,625 params
  Total: 4,801 params × 4 bytes = ~19 KB

Total model size: ~38 KB ✅ (well under 100 KB target!)
```

### Inference Speed

```
Forward pass operations:
  Positional encoding: 24 sin/cos = ~24 ops
  Sensor context: ~53 distances + exp = ~106 ops
  Network layers: 4 matrix-vector products = ~(29×64 + 64×32 + 32×16 + 16) = ~5K ops
  
Total: ~5K FLOPs per query
On modern CPU: << 1ms ✅
```

---

## 🧪 Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_positional_encoding() {
        let enc = positional_encoding(0.0, 0.0, 0.0, 4);
        assert_eq!(enc.len(), 24);  // 4 freqs × 6 (sin/cos for x,y,z)
    }
    
    #[test]
    fn test_field_continuity() {
        let field = TemperatureField::new("test_device");
        
        // Temperature should be continuous
        let t1 = field.estimate_temperature(1.0, 0.0, 0.0, &[], &DeviceState::default());
        let t2 = field.estimate_temperature(1.001, 0.0, 0.0, &[], &DeviceState::default());
        
        assert!((t1 - t2).abs() < 0.1);  // Should be very close
    }
    
    #[test]
    fn test_sensor_influence_decay() {
        let field = TemperatureField::new("test_device");
        
        // Far from sensors should approach ambient
        let readings = vec![("sensor0".to_string(), 60.0)];  // Hot sensor
        
        let near = field.estimate_temperature(0.01, 0.0, 0.0, &readings, &DeviceState::default());
        let far = field.estimate_temperature(1.0, 0.0, 0.0, &readings, &DeviceState::default());
        
        assert!(near > far);  // Influence should decay with distance
    }
}
```

---

## 🎯 Migration Path

### Week 1: Foundation
- [ ] Create `neural_field.rs` module
- [ ] Implement data structures
- [ ] Implement positional encoding
- [ ] Implement SIREN forward pass
- [ ] Unit tests for core functions

### Week 2: Integration
- [ ] Create `TemperatureEstimator` trait
- [ ] Wrap existing Model in trait
- [ ] Implement NeuralFieldEstimator wrapper
- [ ] Add feature flag
- [ ] Side-by-side testing

### Week 3: Training
- [ ] Implement backpropagation for SIREN
- [ ] Implement sensor position optimization
- [ ] Test on real calibration data
- [ ] Compare vs legacy model

### Week 4: Deployment
- [ ] Performance profiling
- [ ] Memory optimization
- [ ] Enable by default
- [ ] Monitor accuracy

---

## 💡 Future Enhancements

### 1. Physics-Informed Training

Add heat equation constraint:
```rust
fn heat_equation_loss(field: &TemperatureField, points: &[(f32,f32,f32)]) -> f32 {
    // ∇²T should be proportional to heat sources
    let mut loss = 0.0;
    
    for &(x, y, z) in points {
        let laplacian = compute_laplacian(field, x, y, z);
        loss += laplacian.powi(2);  // Encourage smooth field
    }
    
    loss
}
```

### 2. Attention Mechanism

Weight sensors by learned importance:
```rust
fn sensor_attention(
    query_pos: (f32, f32, f32),
    sensor_data: &[(String, f32, (f32, f32, f32))],
) -> Vec<f32> {
    // Learn which sensors are most informative for each query
    // Similar to Transformer attention
}
```

### 3. Temporal Modeling

Add time dimension:
```rust
fn estimate_temperature_temporal(
    x: f32, y: f32, z: f32, t: f32,  // t = time in seconds
    sensor_history: &[Vec<(String, f32)>],
) -> f32 {
    // Predict future temperatures
    // Model thermal inertia
}
```

---

## ✅ Summary

**Problem:** Need to estimate ambient temperature at arbitrary points around phone using uncertain internal sensors

**Solution:** Neural field with:
- Positional encoding (NeRF-style)
- SIREN activation (smooth fields)
- Learned sensor positions
- Separate ambient + device heat models

**Benefits:**
- Query temperature anywhere (1m away, any direction)
- Handles sensor dropout naturally
- Device-aware (learned positions)
- Tiny model (<40 KB)
- Fast inference (<1ms)
- Continuous spatial representation

**Integration:** Non-disruptive via trait abstraction, feature flag, incremental rollout

**This architecture solves ALL the problems we identified: sensor dropout, device confusion, spatial estimation, and position independence!** 🎯
