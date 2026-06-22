# Physics-Aware Neural Field Implementation Plan

## 🎯 Core Innovation: Temporal Gradients + Heat Physics

**Key insight from thermal analysis:**
- Phone in case: thermal time constant τ = R × C (20-180s)
- Heat waves propagate through sensors
- CPU spikes create detectable patterns
- Ambient cooling rate reveals true ambient temp

**Our approach:**
1. Track sensor history (120s window)
2. Compute ΔT/Δt for each sensor
3. Learn heat transfer function
4. Predict T(x,y,z) anywhere around phone

---

## 📊 Architecture Design

### Input Features (per query)

**Sensor state (53 values):**
```rust
struct SensorSnapshot {
    temperatures: Vec<f32>,      // Current temps [53]
    gradients: Vec<f32>,          // ΔT/Δt over 5s [53]
    slow_gradients: Vec<f32>,     // ΔT/Δt over 60s [53]
}
```

**Device state (5 values):**
```rust
struct DeviceState {
    cpu_load: f32,           // 0.0 to 1.0
    gpu_load: f32,           // 0.0 to 1.0  
    battery_current: f32,    // Amps (negative = charging)
    screen_brightness: f32,  // 0.0 to 1.0
    charging: f32,           // 0.0 or 1.0
}
```

**Query position (3 values):**
```rust
(x, y, z)  // meters from phone center
```

**Total input size: 53*3 + 5 + 3 = 167 values**

---

### Network Architecture

```
Input: [sensor_temps, gradients_5s, gradients_60s, device_state, query_pos]
         [53, 53, 53, 5, 3] = 167 dims

↓

Positional Encoding (query pos only)
[x,y,z] → [sin(2^0πx), cos(2^0πx), ..., sin(2^4πx), cos(2^4πx)]
3 → 30 dims (5 frequencies × 2 sin/cos × 3 coords)

↓

Combined Input: [167 - 3 + 30] = 194 dims

↓

Layer 1: 194 → 128 (GELU activation)
Layer 2: 128 → 64 (GELU activation)  
Layer 3: 64 → 32 (GELU activation)

↓ ↓ (split into two heads)

Ambient Head: 32 → 1          Device Heat Head: 32 → 1
(external temp)                (internal heat offset)

↓

Final: T(x,y,z) = ambient + device_heat
```

**Model size estimate:**
```
Layer 1: 194 × 128 = 24,832 params
Layer 2: 128 × 64 = 8,192 params
Layer 3: 64 × 32 = 2,048 params
Ambient: 32 × 1 = 32 params
Device: 32 × 1 = 32 params
Biases: ~350 params

Total: ~35,500 params × 4 bytes = 142 KB
```
*Slightly over 100KB target, but worth it for the temporal features*

---

## 🔧 Data Structures

### Main Structure

```rust
pub struct TemperatureField {
    // Network weights
    layer1_weights: Vec<Vec<f32>>,  // [194 × 128]
    layer1_bias: Vec<f32>,           // [128]
    
    layer2_weights: Vec<Vec<f32>>,  // [128 × 64]
    layer2_bias: Vec<f32>,           // [64]
    
    layer3_weights: Vec<Vec<f32>>,  // [64 × 32]
    layer3_bias: Vec<f32>,           // [32]
    
    ambient_weights: Vec<f32>,       // [32 × 1]
    ambient_bias: f32,
    
    device_weights: Vec<f32>,        // [32 × 1]
    device_bias: f32,
    
    // Sensor history for temporal gradients
    sensor_history: VecDeque<SensorSnapshot>,  // 120s @ 1Hz = 120 snapshots
    history_timestamps: VecDeque<f64>,
    
    // Calibration data
    calibration_count: usize,
    last_saved_hash: u64,
}

#[derive(Clone)]
pub struct SensorSnapshot {
    pub timestamp: f64,
    pub temperatures: Vec<f32>,
    pub device_state: DeviceState,
}

#[derive(Clone, Copy)]
pub struct DeviceState {
    pub cpu_load: f32,
    pub gpu_load: f32,
    pub battery_current: f32,
    pub screen_brightness: f32,
    pub charging: bool,
}
```

---

## 🎓 Training Strategy

### Temporal Gradient Computation

```rust
fn compute_gradients(&self, window_seconds: f64) -> Vec<f32> {
    let mut gradients = vec![0.0; 53];
    
    if self.sensor_history.len() < 2 {
        return gradients;
    }
    
    // Find snapshot from `window_seconds` ago
    let now = self.sensor_history.back().unwrap().timestamp;
    let target_time = now - window_seconds;
    
    let past_idx = self.history_timestamps.iter()
        .position(|&t| t >= target_time)
        .unwrap_or(0);
    
    if past_idx == self.sensor_history.len() - 1 {
        return gradients;  // Not enough history
    }
    
    let current = self.sensor_history.back().unwrap();
    let past = &self.sensor_history[past_idx];
    let dt = current.timestamp - past.timestamp;
    
    if dt < 0.1 {
        return gradients;  // Too close in time
    }
    
    // Compute ΔT/Δt for each sensor
    for i in 0..53 {
        let dT = current.temperatures[i] - past.temperatures[i];
        gradients[i] = dT / dt as f32;
    }
    
    gradients
}
```

### Forward Pass

```rust
pub fn estimate_temperature(
    &self,
    x: f32,
    y: f32,
    z: f32,
    current_sensors: &[f32],
    device_state: &DeviceState,
) -> f32 {
    // 1. Compute temporal gradients
    let gradients_5s = self.compute_gradients(5.0);
    let gradients_60s = self.compute_gradients(60.0);
    
    // 2. Positional encoding
    let pos_encoding = self.positional_encoding(x, y, z);
    
    // 3. Combine all inputs
    let mut input = Vec::with_capacity(194);
    input.extend_from_slice(current_sensors);     // 53
    input.extend_from_slice(&gradients_5s);       // 53
    input.extend_from_slice(&gradients_60s);      // 53
    input.push(device_state.cpu_load);            // 1
    input.push(device_state.gpu_load);            // 1
    input.push(device_state.battery_current);     // 1
    input.push(device_state.screen_brightness);   // 1
    input.push(if device_state.charging { 1.0 } else { 0.0 });  // 1
    input.extend_from_slice(&pos_encoding);       // 30
    
    // 4. Forward pass
    let h1 = self.layer_forward(&input, &self.layer1_weights, &self.layer1_bias);
    let h1_act = h1.iter().map(|&x| gelu(x)).collect::<Vec<_>>();
    
    let h2 = self.layer_forward(&h1_act, &self.layer2_weights, &self.layer2_bias);
    let h2_act = h2.iter().map(|&x| gelu(x)).collect::<Vec<_>>();
    
    let h3 = self.layer_forward(&h2_act, &self.layer3_weights, &self.layer3_bias);
    let h3_act = h3.iter().map(|&x| gelu(x)).collect::<Vec<_>>();
    
    // 5. Two-head output
    let ambient = h3_act.iter()
        .zip(self.ambient_weights.iter())
        .map(|(&a, &w)| a * w)
        .sum::<f32>() + self.ambient_bias;
    
    let device_heat = h3_act.iter()
        .zip(self.device_weights.iter())
        .map(|(&a, &w)| a * w)
        .sum::<f32>() + self.device_bias;
    
    // 6. Combine
    ambient + device_heat
}

fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + ((2.0 / std::f32::consts::PI).sqrt() * (x + 0.044715 * x.powi(3))).tanh())
}
```

### Calibration (Backprop)

```rust
pub fn calibrate(
    &mut self,
    current_sensors: &[f32],
    device_state: &DeviceState,
    true_ambient: f32,
    learning_rate: f32,
) {
    // Assume ambient is uniform at 1m from device
    let test_points = [
        (0.0, 1.0, 0.0),   // North
        (0.0, -1.0, 0.0),  // South
        (1.0, 0.0, 0.0),   // East
        (-1.0, 0.0, 0.0),  // West
    ];
    
    for &(x, y, z) in &test_points {
        let predicted = self.estimate_temperature(x, y, z, current_sensors, device_state);
        let error = predicted - true_ambient;
        
        // Backprop (simplified - use autodiff in production)
        // For now: numerical gradients + gradient descent
        
        self.update_weights(error, learning_rate);
    }
    
    self.calibration_count += 1;
}
```

---

## 🔄 Integration with Existing Code

### Phase 1: Add TemperatureField Trait

```rust
// src/temperature_field.rs (NEW FILE)

pub trait TemperatureEstimator {
    fn estimate_ambient(&self, sensors: &[f32], device_state: &DeviceState) -> f32;
    fn calibrate(&mut self, sensors: &[f32], device_state: &DeviceState, target: f32);
    fn update_sensor_history(&mut self, sensors: &[f32], device_state: DeviceState);
}

// Legacy wrapper
pub struct LegacyEstimator {
    model: Model,  // Existing Model struct
}

impl TemperatureEstimator for LegacyEstimator {
    fn estimate_ambient(&self, sensors: &[f32], _device_state: &DeviceState) -> f32 {
        self.model.predict(sensors)
    }
    
    fn calibrate(&mut self, sensors: &[f32], _device_state: &DeviceState, target: f32) {
        self.model.calibrate(sensors, target);
    }
    
    fn update_sensor_history(&mut self, _sensors: &[f32], _device_state: DeviceState) {
        // Legacy model doesn't use history
    }
}

// New neural field
pub struct NeuralFieldEstimator {
    field: TemperatureField,
}

impl TemperatureEstimator for NeuralFieldEstimator {
    fn estimate_ambient(&self, sensors: &[f32], device_state: &DeviceState) -> f32 {
        // Query 1m in each direction, average
        let dirs = [(0.0, 1.0, 0.0), (0.0, -1.0, 0.0), 
                    (1.0, 0.0, 0.0), (-1.0, 0.0, 0.0)];
        
        let sum: f32 = dirs.iter()
            .map(|&(x, y, z)| self.field.estimate_temperature(x, y, z, sensors, device_state))
            .sum();
        
        sum / dirs.len() as f32
    }
    
    fn calibrate(&mut self, sensors: &[f32], device_state: &DeviceState, target: f32) {
        self.field.calibrate(sensors, device_state, target, 0.001);
    }
    
    fn update_sensor_history(&mut self, sensors: &[f32], device_state: DeviceState) {
        self.field.add_snapshot(sensors, device_state);
    }
}
```

### Phase 2: Modify Main Application

```rust
// In main.rs, replace Model with trait object

let estimator: Box<dyn TemperatureEstimator> = if USE_NEURAL_FIELD {
    Box::new(NeuralFieldEstimator {
        field: TemperatureField::new(),
    })
} else {
    Box::new(LegacyEstimator {
        model: Model::load_or_new(...),
    })
};

// In update loop
estimator.update_sensor_history(&sensor_readings, device_state);
let ambient = estimator.estimate_ambient(&sensor_readings, &device_state);

// In calibration
estimator.calibrate(&sensor_readings, &device_state, user_input_temp);
```

---

## 🧪 Testing Strategy

### Unit Tests

```rust
#[test]
fn test_gradient_computation() {
    let mut field = TemperatureField::new();
    
    // Add snapshots with increasing temp
    for i in 0..10 {
        let temps = vec![20.0 + i as f32; 53];
        field.add_snapshot(&temps, DeviceState::default());
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    
    let gradients = field.compute_gradients(5.0);
    
    // Should be ~1.0°C/s (rising 1° per second)
    assert!((gradients[0] - 1.0).abs() < 0.1);
}

#[test]
fn test_spatial_interpolation() {
    let field = TemperatureField::new();
    
    let sensors = vec![60.0; 53];  // Hot sensors
    let device_state = DeviceState {
        cpu_load: 1.0,  // Max load
        ..Default::default()
    };
    
    let near = field.estimate_temperature(0.01, 0.0, 0.0, &sensors, &device_state);
    let far = field.estimate_temperature(1.0, 0.0, 0.0, &sensors, &device_state);
    
    // Farther should be cooler (ambient wins)
    assert!(near > far);
}
```

---

## 📈 Expected Performance

**Advantages over old model:**
1. ✅ **Handles transients:** ΔT/Δt captures heating/cooling
2. ✅ **Device-aware:** CPU/GPU load explains heat sources
3. ✅ **Spatial queries:** Can estimate temp anywhere
4. ✅ **Physics-informed:** Heat flow patterns emerge from gradients
5. ✅ **Case-robust:** Longer time constants learned automatically

**Inference speed:**
- Forward pass: ~200K FLOPs
- On modern CPU: **< 0.5ms** ✅

**Memory:**
- Model: 142 KB
- History: 120 snapshots × 53 sensors × 4 bytes = ~25 KB
- **Total: ~170 KB** (reasonable)

---

## 🚀 Implementation Order

### Week 1: Foundation
1. Create `temperature_field.rs` module
2. Implement `TemperatureField` struct
3. Add sensor history tracking
4. Implement gradient computation
5. Unit tests for gradients

### Week 2: Network
1. Implement positional encoding
2. Implement forward pass (GELU)
3. Implement two-head output
4. Add weight initialization (Xavier)
5. Unit tests for forward pass

### Week 3: Training
1. Implement backpropagation
2. Add calibration logic
3. Serialize/deserialize
4. Integration tests

### Week 4: Integration
1. Create `TemperatureEstimator` trait
2. Wrap legacy Model
3. Implement NeuralFieldEstimator
4. Feature flag in main.rs
5. Side-by-side comparison

---

## 💡 Physics Insights to Leverage

### Heat Equation Patterns

**CPU spike signature:**
```
t=0s:  CPU sensor jumps +20°C instantly
t=2s:  Nearby sensors +5°C (heat diffusion)
t=10s: All sensors +2°C (chassis equilibrium)
t=60s: Gradual decay as heat escapes
```

**The gradients reveal:**
- Fast gradients (5s) → recent activity
- Slow gradients (60s) → ambient cooling rate

**Network learns:**
```
If ΔT/Δt(CPU) > 2°C/s  →  CPU is heating
If ΔT/Δt(all) < 0.1°C/s → ambient cooling dominates
```

### Thermal Time Constants (with case)

```
Bare phone: τ ~ 10s
Thin case:  τ ~ 30s
Thick case: τ ~ 120s
```

**Why 60s gradient window?**
- Captures full decay curve
- Works for all case types
- Reveals true ambient cooling

---

## ✅ Summary

**This architecture:**
- Uses **temporal gradients** to detect heat flow
- Separates **ambient** from **device heat**
- Handles **any case type** (learns time constants)
- Predicts **T(x,y,z)** anywhere
- **170 KB total**, **<0.5ms inference**

**Next step:** Start implementing `temperature_field.rs` module with sensor history tracking and gradient computation! 🚀
