// src/temperature_field.rs
// Physics-aware 3D temperature field with temporal gradients

use std::collections::VecDeque;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use crate::world_state::DeviceState;

/// Trait for temperature estimation (allows swapping estimators)
pub trait TemperatureEstimator {
    /// Estimate ambient temperature from sensor readings
    fn estimate_ambient(&mut self, sensors: &[f32], device_state: &DeviceState) -> f32;
    
    /// Calibrate with ground truth ambient temperature
    fn calibrate(&mut self, sensors: &[f32], device_state: &DeviceState, target: f32);
    
    /// Update sensor history (for temporal gradient models)
    fn update_sensor_history(&mut self, sensors: &[f32], device_state: &DeviceState);
    
    /// Get calibration count
    fn get_calibration_count(&self) -> usize;
    
    /// Predict from raw sensors (legacy compatibility)
    fn predict(&mut self, sensors: &[f32]) -> f32 {
        self.estimate_ambient(sensors, &crate::world_state::DeviceState::default())
    }
}

/// Snapshot of sensor readings at a point in time
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SensorSnapshot {
    pub timestamp: f64,           // Unix timestamp (seconds)
    pub temperatures: Vec<f32>,   // Sensor temperatures
    pub device_state: DeviceState,
}

/// Activations saved during forward pass for backpropagation
struct ForwardActivations {
    input: Vec<f32>,
    h1_pre: Vec<f32>,
    h1: Vec<f32>,
    h2_pre: Vec<f32>,
    h2: Vec<f32>,
    h3_pre: Vec<f32>,
    h3: Vec<f32>,
    ambient: f32,
    device_heat: f32,
}

impl SensorSnapshot {
    pub fn new(temperatures: Vec<f32>, device_state: DeviceState) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        
        Self {
            timestamp,
            temperatures,
            device_state,
        }
    }
}

/// Physics-aware neural field for temperature estimation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemperatureField {
    // Network architecture
    pub num_sensors: usize,
    
    // Layer 1: input_size → 128
    layer1_weights: Vec<Vec<f32>>,  // [input_size × 128]
    layer1_bias: Vec<f32>,          // [128]
    
    // Layer 2: 128 → 64
    layer2_weights: Vec<Vec<f32>>,  // [128 × 64]
    layer2_bias: Vec<f32>,          // [64]
    
    // Layer 3: 64 → 32
    layer3_weights: Vec<Vec<f32>>,  // [64 × 32]
    layer3_bias: Vec<f32>,          // [32]
    
    // Output heads
    ambient_weights: Vec<f32>,      // [32 × 1]
    ambient_bias: f32,
    
    device_weights: Vec<f32>,       // [32 × 1]
    device_bias: f32,
    
    // Sensor history for temporal gradients
    #[serde(skip)]  // Don't serialize history (too large)
    sensor_history: VecDeque<SensorSnapshot>,
    
    // Training state
    calibration_count: usize,
    last_prediction: f32,  // Fallback for errors
    
    // Positional encoding settings
    pos_encoding_freqs: usize,  // Number of frequency bands (default 5)
}

impl TemperatureField {
    /// Create a new temperature field with Xavier initialization
    pub fn new(num_sensors: usize) -> Self {
        let pos_encoding_freqs = 5;
        let pos_encoding_dims = pos_encoding_freqs * 2 * 3;  // freq × (sin,cos) × (x,y,z) = 30
        
        // Input size: sensors + gradients_5s + gradients_60s + device_state + pos_encoding
        // = num_sensors + num_sensors + num_sensors + 5 + 30
        let input_size = num_sensors * 3 + 5 + pos_encoding_dims;
        
        println!("🏗️  Initializing TemperatureField:");
        println!("   Sensors: {}", num_sensors);
        println!("   Input size: {} ({}×3 sensors + 5 device + 30 pos_enc)", input_size, num_sensors);
        println!("   Architecture: {} → 128 → 64 → 32 → 2", input_size);
        
        let mut field = Self {
            num_sensors,
            
            layer1_weights: Vec::new(),
            layer1_bias: vec![0.0; 128],
            
            layer2_weights: Vec::new(),
            layer2_bias: vec![0.0; 64],
            
            layer3_weights: Vec::new(),
            layer3_bias: vec![0.0; 32],
            
            ambient_weights: vec![0.0; 32],
            ambient_bias: 0.0,
            
            device_weights: vec![0.0; 32],
            device_bias: 0.0,
            
            sensor_history: VecDeque::new(),
            
            calibration_count: 0,
            last_prediction: 20.0,  // Default room temp
            
            pos_encoding_freqs,
        };
        
        // Xavier initialization
        field.initialize_weights(input_size);
        
        println!("✓ TemperatureField initialized");
        
        field
    }
    
    /// Initialize weights using Xavier/Glorot initialization
    fn initialize_weights(&mut self, input_size: usize) {
        // Simple pseudo-random based on current time
        let mut seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        
        let mut rng = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((seed >> 32) as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        
        // Layer 1: input_size → 128 (normal Xavier, not scaled down)
        let xavier_1 = (2.0 / (input_size + 128) as f32).sqrt();  // He initialization for GELU
        self.layer1_weights = (0..input_size)
            .map(|_| (0..128).map(|_| rng() * xavier_1).collect())
            .collect();
        
        // Layer 2: 128 → 64
        let xavier_2 = (2.0 / (128 + 64) as f32).sqrt();
        self.layer2_weights = (0..128)
            .map(|_| (0..64).map(|_| rng() * xavier_2).collect())
            .collect();
        
        // Layer 3: 64 → 32
        let xavier_3 = (2.0 / (64 + 32) as f32).sqrt();
        self.layer3_weights = (0..64)
            .map(|_| (0..32).map(|_| rng() * xavier_3).collect())
            .collect();
        
        // Output heads: 32 → 1 (normal Xavier, not too small)
        let xavier_out = (2.0 / (32 + 1) as f32).sqrt();
        self.ambient_weights = (0..32).map(|_| rng() * xavier_out).collect();
        self.device_weights = (0..32).map(|_| rng() * xavier_out).collect();
        
        // Initialize biases to zero - let the network learn the offset
        // Starting at 20°C was causing mode collapse!
        self.ambient_bias = 0.0;
        self.device_bias = 0.0;
        
        println!("✓ Weights initialized with He method (for GELU activation)");
        println!("   Xavier scales: L1={:.4}, L2={:.4}, L3={:.4}, Out={:.4}", 
            xavier_1, xavier_2, xavier_3, xavier_out);
        println!("   Initial biases: ambient={:.1}°C, device={:.1}°C", 
            self.ambient_bias, self.device_bias);
    }
    
    /// Add a sensor snapshot to history
    pub fn add_snapshot(&mut self, temperatures: &[f32], device_state: &DeviceState) {
        if temperatures.len() != self.num_sensors {
            eprintln!("⚠️  Sensor count mismatch: expected {}, got {}", 
                self.num_sensors, temperatures.len());
            return;
        }
        
        let snapshot = SensorSnapshot::new(temperatures.to_vec(), device_state.clone());
        self.sensor_history.push_back(snapshot);
        
        // Keep 120 seconds of history (at ~1 Hz)
        const MAX_HISTORY: usize = 120;
        while self.sensor_history.len() > MAX_HISTORY {
            self.sensor_history.pop_front();
        }
    }
    
    /// Compute temporal gradients ΔT/Δt over specified window
    pub fn compute_gradients(&self, window_seconds: f64) -> Vec<f32> {
        let mut gradients = vec![0.0; self.num_sensors];
        
        if self.sensor_history.len() < 2 {
            return gradients;  // Not enough history
        }
        
        let current = self.sensor_history.back().unwrap();
        let target_time = current.timestamp - window_seconds;
        
        // Find snapshot closest to target_time
        let past_idx = self.sensor_history.iter()
            .position(|s| s.timestamp >= target_time)
            .unwrap_or(0);
        
        if past_idx >= self.sensor_history.len() - 1 {
            return gradients;  // Not enough separation
        }
        
        let past = &self.sensor_history[past_idx];
        let dt = current.timestamp - past.timestamp;
        
        if dt < 0.1 {
            return gradients;  // Too close in time
        }
        
        // Compute ΔT/Δt for each sensor
        for i in 0..self.num_sensors {
            let d_t = current.temperatures[i] - past.temperatures[i];
            gradients[i] = d_t / dt as f32;
        }
        
        gradients
    }
    
    /// Positional encoding for spatial coordinates (NeRF-style)
    pub fn positional_encoding(&self, x: f32, y: f32, z: f32) -> Vec<f32> {
        let mut encoding = Vec::with_capacity(self.pos_encoding_freqs * 6);
        
        for i in 0..self.pos_encoding_freqs {
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
        
        encoding
    }
    
    /// GELU activation function (smooth, better than ReLU for gradients)
    #[inline]
    fn gelu(x: f32) -> f32 {
        0.5 * x * (1.0 + ((2.0 / std::f32::consts::PI).sqrt() 
            * (x + 0.044715 * x.powi(3))).tanh())
    }
    
    /// Forward pass through a layer
    fn layer_forward(&self, input: &[f32], weights: &[Vec<f32>], bias: &[f32]) -> Vec<f32> {
        let output_size = bias.len();
        let mut output = vec![0.0; output_size];
        
        for i in 0..output_size {
            let mut sum = bias[i];
            for (j, &x) in input.iter().enumerate() {
                if j < weights.len() && i < weights[j].len() {
                    sum += x * weights[j][i];
                }
            }
            output[i] = sum;
        }
        
        output
    }
    
    /// Estimate temperature at arbitrary 3D point
    pub fn estimate_temperature(
        &mut self,
        x: f32,
        y: f32,
        z: f32,
        current_sensors: &[f32],
        device_state: &crate::world_state::DeviceState,
    ) -> f32 {
        // Validate sensor count
        if current_sensors.len() != self.num_sensors {
            eprintln!("⚠️  Sensor mismatch in estimate: expected {}, got {}", 
                self.num_sensors, current_sensors.len());
            return self.last_prediction;
        }
        
        // 1. Compute temporal gradients
        let gradients_5s = self.compute_gradients(5.0);
        let gradients_60s = self.compute_gradients(60.0);
        
        // 2. Positional encoding
        let pos_encoding = self.positional_encoding(x, y, z);
        
        // 3. Build input vector with normalization (MUST match training!)
        let mut input = Vec::with_capacity(self.num_sensors * 3 + 5 + pos_encoding.len());
        
        // Normalize sensor temperatures (typical range: 20-80°C → -1 to 1)
        for &temp in current_sensors {
            let normalized = (temp - 50.0) / 30.0;  // Center at 50°C, scale by ±30°C
            input.push(normalized.clamp(-3.0, 3.0));
        }
        
        // Normalize fast gradients (5s window)
        for &grad in &gradients_5s {
            input.push((grad / 2.0).clamp(-3.0, 3.0));
        }
        
        // Normalize slow gradients (60s window)
        for &grad in &gradients_60s {
            input.push((grad / 0.5).clamp(-3.0, 3.0));
        }
        
        // Device state normalization (5 values to match network input size)
        input.push(device_state.cpu_load.clamp(0.0, 1.0));
        input.push(device_state.battery_level.clamp(0.0, 1.0));
        input.push(device_state.screen_brightness.clamp(0.0, 1.0));
        input.push(if device_state.charging { 1.0 } else { 0.0 });
        input.push(0.0);  // Reserved slot (was gpu_load)
        
        // Add positional encoding
        input.extend_from_slice(&pos_encoding);
        
        // 4. Forward pass through network
        let h1 = self.layer_forward(&input, &self.layer1_weights, &self.layer1_bias);
        let h1_act: Vec<f32> = h1.iter().map(|&x| Self::gelu(x)).collect();
        
        let h2 = self.layer_forward(&h1_act, &self.layer2_weights, &self.layer2_bias);
        let h2_act: Vec<f32> = h2.iter().map(|&x| Self::gelu(x)).collect();
        
        let h3 = self.layer_forward(&h2_act, &self.layer3_weights, &self.layer3_bias);
        let h3_act: Vec<f32> = h3.iter().map(|&x| Self::gelu(x)).collect();
        
        // 5. Two-head output
        let ambient: f32 = h3_act.iter()
            .zip(self.ambient_weights.iter())
            .map(|(&a, &w)| a * w)
            .sum::<f32>() + self.ambient_bias;
        
        let device_heat: f32 = h3_act.iter()
            .zip(self.device_weights.iter())
            .map(|(&a, &w)| a * w)
            .sum::<f32>() + self.device_bias;
        
        // 6. Combine predictions
        let output = ambient + device_heat;
        
        // 7. Sanity check
        if output.is_nan() || output.is_infinite() {
            eprintln!("⚠️  NaN/Inf in prediction, using last value");
            return self.last_prediction;
        }
        
        if output.abs() > 200.0 {
            eprintln!("⚠️  Extreme prediction: {:.1}°C, using last value", output);
            return self.last_prediction;
        }
        
        self.last_prediction = output;
        output
    }
    
    /// Estimate ambient temperature (average of cardinal directions at 1m)
    pub fn estimate_ambient(
        &mut self,
        current_sensors: &[f32],
        device_state: &DeviceState,
    ) -> f32 {
        let directions = [
            (0.0, 1.0, 0.0),   // North
            (0.0, -1.0, 0.0),  // South
            (1.0, 0.0, 0.0),   // East
            (-1.0, 0.0, 0.0),  // West
        ];
        
        let sum: f32 = directions.iter()
            .map(|&(x, y, z)| self.estimate_temperature(x, y, z, current_sensors, device_state))
            .sum();
        
        sum / directions.len() as f32
    }
    
    /// Calibrate the field with ground truth ambient temperature
    pub fn calibrate(
        &mut self,
        current_sensors: &[f32],
        device_state: &DeviceState,
        true_ambient: f32,
        base_learning_rate: f32,
    ) {
        // Validate sensor count
        if current_sensors.len() != self.num_sensors {
            eprintln!("❌ SENSOR MISMATCH in calibrate: expected {}, got {}", 
                self.num_sensors, current_sensors.len());
            return;
        }
        
        println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("🎯 Calibration #{}", self.calibration_count);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("📊 Input:");
        println!("   Sensor range: [{:.1}, {:.1}]°C", 
            current_sensors.iter().cloned().fold(f32::INFINITY, f32::min),
            current_sensors.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
        println!("   History size: {} snapshots", self.sensor_history.len());
        println!("   Target ambient: {:.1}°C", true_ambient);
        
        // Train on multiple spatial points (assume ambient is uniform at 1m)
        let test_points = [
            ("North", 0.0, 1.0, 0.0),
            ("South", 0.0, -1.0, 0.0),
            ("East", 1.0, 0.0, 0.0),
            ("West", -1.0, 0.0, 0.0),
        ];
        
        let mut total_error = 0.0;
        let mut predictions = Vec::new();
        let mut trained_count = 0;
        let mut skipped_count = 0;
        
        println!("\n📍 Spatial predictions:");
        for &(name, x, y, z) in &test_points {
            // Forward pass with saved activations
            let (predicted, activations) = self.forward_with_activations(
                x, y, z, current_sensors, device_state
            );
            
            predictions.push(predicted);
            
            let error = predicted - true_ambient;
            total_error += error.abs();
            
            println!("   {}: {:.1}°C (error: {:+.1}°C)", 
                name, predicted, error);
            
            // Skip training if error is catastrophic (model diverged)
            if error.abs() > 100.0 {
                eprintln!("   ⚠️  SKIPPED - Error too large (> 100°C)");
                skipped_count += 1;
                continue;
            }
            
            // Adaptive learning rate based on error magnitude
            let learning_rate = self.compute_adaptive_lr(error.abs(), base_learning_rate);
            
            // Backpropagation
            self.backpropagate(error, learning_rate, &activations);
            trained_count += 1;
        }
        
        let avg_error = total_error / test_points.len() as f32;
        let avg_prediction = predictions.iter().sum::<f32>() / predictions.len() as f32;
        let min_pred = predictions.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_pred = predictions.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let pred_variance = max_pred - min_pred;
        
        println!("\n📈 Results:");
        println!("   Average prediction: {:.1}°C", avg_prediction);
        println!("   Prediction range: [{:.1}, {:.1}]°C (variance: {:.1}°C)", 
            min_pred, max_pred, pred_variance);
        println!("   Average error: {:.1}°C", avg_error);
        println!("   Training: {} points trained, {} skipped", trained_count, skipped_count);
        
        // Show gradient flow diagnostics for first few calibrations
        if self.calibration_count < 5 {
            println!("\n🔬 Gradient diagnostics:");
            println!("   Ambient bias: {:.4}", self.ambient_bias);
            println!("   Device bias: {:.4}", self.device_bias);
            
            // Sample some weights to show they're changing
            if !self.ambient_weights.is_empty() {
                let amb_mean = self.ambient_weights.iter().sum::<f32>() / self.ambient_weights.len() as f32;
                let amb_max = self.ambient_weights.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let amb_min = self.ambient_weights.iter().cloned().fold(f32::INFINITY, f32::min);
                println!("   Ambient weights: mean={:.4}, range=[{:.4}, {:.4}]", amb_mean, amb_min, amb_max);
            }
        }
        
        if trained_count == 0 {
            eprintln!("\n❌ WARNING: No training occurred! Model not updated.");
        } else {
            println!("   ✓ Model updated successfully");
        }
        
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
        
        self.calibration_count += 1;
    }
    
    /// Compute adaptive learning rate based on error magnitude
    fn compute_adaptive_lr(&self, error: f32, base_lr: f32) -> f32 {
        let lr = if error < 1.0 {
            base_lr * 0.1  // Fine-tuning phase (very accurate)
        } else if error < 3.0 {
            base_lr * 0.5  // Refinement phase (good accuracy)
        } else if error < 10.0 {
            base_lr * 1.0  // Normal learning
        } else if error < 30.0 {
            base_lr * 2.0  // Aggressive (large error)
        } else if error < 100.0 {
            base_lr * 3.0  // Very aggressive (model far off)
        } else {
            base_lr * 0.01 // Emergency brake (model unstable)
        };
        
        if self.calibration_count < 5 {
            println!("   📐 Adaptive LR: {:.6} (base={:.6}, error={:.1}°C)", 
                lr, base_lr, error);
        }
        
        lr
    }
    
    /// Forward pass that saves activations for backprop
    fn forward_with_activations(
        &self,
        x: f32,
        y: f32,
        z: f32,
        current_sensors: &[f32],
        device_state: &DeviceState,
    ) -> (f32, ForwardActivations) {
        // 1. Compute temporal gradients
        let gradients_5s = self.compute_gradients(5.0);
        let gradients_60s = self.compute_gradients(60.0);
        
        // 2. Positional encoding
        let pos_encoding = self.positional_encoding(x, y, z);
        
        // 3. Build input vector with normalization
        let mut input = Vec::with_capacity(self.num_sensors * 3 + 5 + pos_encoding.len());
        
        // Normalize sensor temperatures (typical range: 20-80°C → -1 to 1)
        for &temp in current_sensors {
            let normalized = (temp - 50.0) / 30.0;  // Center at 50°C, scale by ±30°C
            input.push(normalized.clamp(-3.0, 3.0));
        }
        
        // Normalize gradients (typical range: -2 to +2 °C/s)
        for &grad in &gradients_5s {
            input.push((grad / 2.0).clamp(-3.0, 3.0));
        }
        for &grad in &gradients_60s {
            input.push((grad / 0.5).clamp(-3.0, 3.0));
        }
        
        // Device state already in [0, 1] range (5 values to match network input size)
        input.push(device_state.cpu_load.clamp(0.0, 1.0));
        input.push(device_state.battery_level.clamp(0.0, 1.0));
        input.push(device_state.screen_brightness.clamp(0.0, 1.0));
        input.push(if device_state.charging { 1.0 } else { 0.0 });
        input.push(0.0);  // Reserved slot (was gpu_load)
        
        // Positional encoding already in [-1, 1]
        input.extend_from_slice(&pos_encoding);
        
        // 4. Layer 1
        let h1_pre = self.layer_forward(&input, &self.layer1_weights, &self.layer1_bias);
        let h1 = h1_pre.iter().map(|&x| Self::gelu(x)).collect::<Vec<_>>();
        
        // 5. Layer 2
        let h2_pre = self.layer_forward(&h1, &self.layer2_weights, &self.layer2_bias);
        let h2 = h2_pre.iter().map(|&x| Self::gelu(x)).collect::<Vec<_>>();
        
        // 6. Layer 3
        let h3_pre = self.layer_forward(&h2, &self.layer3_weights, &self.layer3_bias);
        let h3 = h3_pre.iter().map(|&x| Self::gelu(x)).collect::<Vec<_>>();
        
        // 7. Output heads
        let ambient: f32 = h3.iter()
            .zip(self.ambient_weights.iter())
            .map(|(&a, &w)| a * w)
            .sum::<f32>() + self.ambient_bias;
        
        let device_heat: f32 = h3.iter()
            .zip(self.device_weights.iter())
            .map(|(&a, &w)| a * w)
            .sum::<f32>() + self.device_bias;
        
        let output = ambient + device_heat;
        
        // Save activations for backprop
        let activations = ForwardActivations {
            input,
            h1_pre,
            h1,
            h2_pre,
            h2,
            h3_pre,
            h3,
            ambient,
            device_heat,
        };
        
        (output, activations)
    }
    
    /// Backpropagation through the network
    fn backpropagate(
        &mut self,
        error: f32,
        learning_rate: f32,
        acts: &ForwardActivations,
    ) {
        // L2 regularization strength (reduced to allow faster learning)
        const L2_LAMBDA: f32 = 0.00001;  // Was 0.0001, now 10x smaller
        
        // Output gradient (MSE derivative: 2 * error)
        let d_output = 2.0 * error;
        
        // Both heads get same gradient (they sum to output)
        let d_ambient = d_output;
        let d_device = d_output;
        
        // Gradients for output layer weights
        let mut d_h3 = vec![0.0; acts.h3.len()];
        
        for i in 0..acts.h3.len() {
            d_h3[i] = d_ambient * self.ambient_weights[i] + d_device * self.device_weights[i];
        }
        
        // Update output heads with gradient clipping and L2 regularization
        const GRAD_CLIP: f32 = 10.0;
        
        for i in 0..self.ambient_weights.len() {
            let grad = Self::clip_gradient(d_ambient * acts.h3[i], GRAD_CLIP);
            let l2_penalty = L2_LAMBDA * self.ambient_weights[i];
            self.ambient_weights[i] -= learning_rate * (grad + l2_penalty);
        }
        let bias_grad = Self::clip_gradient(d_ambient, GRAD_CLIP);
        self.ambient_bias -= learning_rate * bias_grad;  // No L2 on biases
        
        for i in 0..self.device_weights.len() {
            let grad = Self::clip_gradient(d_device * acts.h3[i], GRAD_CLIP);
            let l2_penalty = L2_LAMBDA * self.device_weights[i];
            self.device_weights[i] -= learning_rate * (grad + l2_penalty);
        }
        let bias_grad = Self::clip_gradient(d_device, GRAD_CLIP);
        self.device_bias -= learning_rate * bias_grad;  // No L2 on biases
        
        // Layer 3 gradients (through GELU)
        for i in 0..d_h3.len() {
            d_h3[i] *= Self::gelu_derivative(acts.h3_pre[i]);
        }
        
        // Update layer 3 weights with gradient clipping and L2 regularization
        for i in 0..self.layer3_bias.len() {
            let grad = Self::clip_gradient(d_h3[i], GRAD_CLIP);
            self.layer3_bias[i] -= learning_rate * grad;
            
            for j in 0..acts.h2.len() {
                if j < self.layer3_weights.len() && i < self.layer3_weights[j].len() {
                    let weight_grad = Self::clip_gradient(d_h3[i] * acts.h2[j], GRAD_CLIP);
                    let l2_penalty = L2_LAMBDA * self.layer3_weights[j][i];
                    self.layer3_weights[j][i] -= learning_rate * (weight_grad + l2_penalty);
                }
            }
        }
        
        // Layer 2 gradients
        let mut d_h2 = vec![0.0; acts.h2.len()];
        for i in 0..acts.h2.len() {
            for j in 0..d_h3.len() {
                if i < self.layer3_weights.len() && j < self.layer3_weights[i].len() {
                    d_h2[i] += d_h3[j] * self.layer3_weights[i][j];
                }
            }
            d_h2[i] *= Self::gelu_derivative(acts.h2_pre[i]);
        }
        
        // Update layer 2 weights with gradient clipping and L2 regularization
        for i in 0..self.layer2_bias.len() {
            let grad = Self::clip_gradient(d_h2[i], GRAD_CLIP);
            self.layer2_bias[i] -= learning_rate * grad;
            
            for j in 0..acts.h1.len() {
                if j < self.layer2_weights.len() && i < self.layer2_weights[j].len() {
                    let weight_grad = Self::clip_gradient(d_h2[i] * acts.h1[j], GRAD_CLIP);
                    let l2_penalty = L2_LAMBDA * self.layer2_weights[j][i];
                    self.layer2_weights[j][i] -= learning_rate * (weight_grad + l2_penalty);
                }
            }
        }
        
        // Layer 1 gradients
        let mut d_h1 = vec![0.0; acts.h1.len()];
        for i in 0..acts.h1.len() {
            for j in 0..d_h2.len() {
                if i < self.layer2_weights.len() && j < self.layer2_weights[i].len() {
                    d_h1[i] += d_h2[j] * self.layer2_weights[i][j];
                }
            }
            d_h1[i] *= Self::gelu_derivative(acts.h1_pre[i]);
        }
        
        // Update layer 1 weights with gradient clipping and L2 regularization
        for i in 0..self.layer1_bias.len() {
            let grad = Self::clip_gradient(d_h1[i], GRAD_CLIP);
            self.layer1_bias[i] -= learning_rate * grad;
            
            for j in 0..acts.input.len() {
                if j < self.layer1_weights.len() && i < self.layer1_weights[j].len() {
                    let weight_grad = Self::clip_gradient(d_h1[i] * acts.input[j], GRAD_CLIP);
                    let l2_penalty = L2_LAMBDA * self.layer1_weights[j][i];
                    self.layer1_weights[j][i] -= learning_rate * (weight_grad + l2_penalty);
                }
            }
        }
        
        // Validate weights after update (safety check)
        self.validate_and_fix_weights();
    }
    
    /// Validate weights and fix if necessary
    fn validate_and_fix_weights(&mut self) {
        let mut issues_found = 0;
        
        // Check layer 1
        for row in &mut self.layer1_weights {
            for w in row {
                if !w.is_finite() {
                    *w = 0.0;
                    issues_found += 1;
                }
            }
        }
        
        // Check layer 2
        for row in &mut self.layer2_weights {
            for w in row {
                if !w.is_finite() {
                    *w = 0.0;
                    issues_found += 1;
                }
            }
        }
        
        // Check layer 3
        for row in &mut self.layer3_weights {
            for w in row {
                if !w.is_finite() {
                    *w = 0.0;
                    issues_found += 1;
                }
            }
        }
        
        // Check output heads
        for w in &mut self.ambient_weights {
            if !w.is_finite() {
                *w = 0.0;
                issues_found += 1;
            }
        }
        for w in &mut self.device_weights {
            if !w.is_finite() {
                *w = 0.0;
                issues_found += 1;
            }
        }
        
        if issues_found > 0 {
            eprintln!("⚠️ Fixed {} non-finite weights after backprop", issues_found);
        }
    }
    
    /// GELU derivative for backpropagation
    #[inline]
    fn gelu_derivative(x: f32) -> f32 {
        // Clamp input to prevent overflow/NaN
        // GELU saturates outside [-10, 10] anyway
        let x = x.clamp(-10.0, 10.0);
        
        // Approximate GELU derivative
        // d/dx[GELU(x)] ≈ 0.5 * tanh(z) + (0.5 * x * sech²(z)) * dz/dx
        // where z = sqrt(2/π) * (x + 0.044715 * x³)
        
        let c = (2.0 / std::f32::consts::PI).sqrt();
        let x_cubed = x * x * x;
        let z = c * (x + 0.044715 * x_cubed);
        let tanh_z = z.tanh();
        let sech2_z = 1.0 - tanh_z * tanh_z;
        
        let dz_dx = c * (1.0 + 3.0 * 0.044715 * x * x);
        let derivative = 0.5 * (1.0 + tanh_z) + 0.5 * x * dz_dx * sech2_z;
        
        // Final NaN check (should never trigger with clamped input)
        if derivative.is_finite() {
            derivative
        } else {
            // Fallback to ReLU derivative if somehow we got NaN
            if x > 0.0 { 1.0 } else { 0.0 }
        }
    }
    
    /// Clip gradient to prevent explosion
    #[inline]
    fn clip_gradient(grad: f32, max_norm: f32) -> f32 {
        if !grad.is_finite() {
            eprintln!("⚠️ Non-finite gradient detected: {}", grad);
            return 0.0;
        }
        grad.clamp(-max_norm, max_norm)
    }
    
    /// Get calibration count
    pub fn get_calibration_count(&self) -> usize {
        self.calibration_count
    }
    
    /// Get number of history snapshots
    pub fn get_history_size(&self) -> usize {
        self.sensor_history.len()
    }
    
    /// Get expected number of sensors
    pub fn get_num_sensors(&self) -> usize {
        self.num_sensors
    }
    
    /// Save neural field to JSON file
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Serialization error: {}", e))?;
        
        std::fs::write(path, json)
            .map_err(|e| format!("File write error: {}", e))?;
        
        Ok(())
    }
    
    /// Load neural field from JSON file
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("File read error: {}", e))?;
        
        let field: Self = serde_json::from_str(&json)
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        Ok(field)
    }
    
    /// Bootstrap training from existing calibration data
    pub fn bootstrap_from_calibrations(
        &mut self,
        calibrations: &[(Vec<f32>, f32)],  // (sensors, target_ambient)
        learning_rate: f32,
    ) -> Result<(), String> {
        if calibrations.is_empty() {
            return Err("No calibrations provided for bootstrap".to_string());
        }
        
        println!("\n╔═══════════════════════════════════════════╗");
        println!("║  🚀 BOOTSTRAP TRAINING                    ║");
        println!("╚═══════════════════════════════════════════╝");
        println!("📚 Dataset: {} calibrations", calibrations.len());
        println!("🎯 Target: Learn from historical data");
        println!("⚙️  Learning rate: {:.6}", learning_rate);
        
        // Run multiple epochs over the dataset
        let epochs = if calibrations.len() < 10 {
            20  // More epochs for small datasets
        } else if calibrations.len() < 50 {
            10
        } else {
            5   // Fewer epochs for large datasets
        };
        
        println!("\n📈 Training for {} epochs...\n", epochs);
        
        for epoch in 0..epochs {
            let mut epoch_loss = 0.0;
            let mut count = 0;
            
            // Shuffle order each epoch (simple shuffle by reversing every other epoch)
            let iter: Box<dyn Iterator<Item = &(Vec<f32>, f32)>> = if epoch % 2 == 0 {
                Box::new(calibrations.iter())
            } else {
                Box::new(calibrations.iter().rev())
            };
            
            for (sensors, target) in iter {
                if sensors.len() != self.num_sensors {
                    continue;  // Skip mismatched sensor counts
                }
                
                let device_state = crate::world_state::DeviceState::default();
                
                // Get prediction before training
                let pred_before = self.estimate_ambient(sensors, &device_state);
                
                // Train on this sample
                self.calibrate(sensors, &device_state, *target, learning_rate);
                
                // Track loss
                epoch_loss += (pred_before - target).abs();
                count += 1;
            }
            
            let avg_loss = if count > 0 { epoch_loss / count as f32 } else { 0.0 };
            
            println!("   Epoch {:2}/{}: avg_loss={:6.2}°C  (trained on {} samples)", 
                epoch + 1, epochs, avg_loss, count);
            
            // Early stopping if converged
            if avg_loss < 0.5 {
                println!("\n✨ Converged early! Loss < 0.5°C");
                break;
            }
        }
        
        println!("\n╔═══════════════════════════════════════════╗");
        println!("║  ✅ BOOTSTRAP COMPLETE                    ║");
        println!("╚═══════════════════════════════════════════╝");
        println!("📊 Model calibration count: {}", self.calibration_count);
        println!("🎯 Ready for deployment!\n");
        
        Ok(())
    }
}

/// Implement the TemperatureEstimator trait for TemperatureField
impl TemperatureEstimator for TemperatureField {
    fn estimate_ambient(&mut self, sensors: &[f32], device_state: &crate::world_state::DeviceState) -> f32 {
        self.estimate_ambient(sensors, device_state)
    }
    
    fn calibrate(&mut self, sensors: &[f32], device_state: &crate::world_state::DeviceState, target: f32) {
        self.calibrate(sensors, device_state, target, 0.001)
    }
    
    fn update_sensor_history(&mut self, sensors: &[f32], device_state: &crate::world_state::DeviceState) {
        self.add_snapshot(sensors, device_state);
    }
    
    fn get_calibration_count(&self) -> usize {
        self.calibration_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_temperature_field_creation() {
        let field = TemperatureField::new(53);
        assert_eq!(field.num_sensors, 53);
        assert_eq!(field.calibration_count, 0);
    }
    
    #[test]
    fn test_add_snapshot() {
        let mut field = TemperatureField::new(53);
        let temps = vec![25.0; 53];
        let state = crate::world_state::DeviceState::default();
        
        field.add_snapshot(&temps, &state);
        assert_eq!(field.get_history_size(), 1);
        
        // Add more
        for _ in 0..10 {
            field.add_snapshot(&temps, &state);
        }
        assert_eq!(field.get_history_size(), 11);
    }
    
    #[test]
    fn test_gradient_computation() {
        let mut field = TemperatureField::new(53);
        let state = crate::world_state::DeviceState::default();
        
        // Add snapshots with increasing temperature
        for i in 0..10 {
            let temps = vec![20.0 + i as f32; 53];
            field.add_snapshot(&temps, &state);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        
        let gradients = field.compute_gradients(0.5);
        
        // Should detect rising temperature
        // (approximately 10°C / 1s = 10°C/s, but window is 0.5s)
        assert!(gradients[0] > 0.0);
    }
    
    #[test]
    fn test_positional_encoding() {
        let field = TemperatureField::new(53);
        let encoding = field.positional_encoding(0.5, 0.0, -0.5);
        
        // Should have 5 frequencies × 2 (sin/cos) × 3 (x,y,z) = 30 values
        assert_eq!(encoding.len(), 30);
    }
    
    #[test]
    fn test_estimate_temperature() {
        let mut field = TemperatureField::new(53);
        let sensors = vec![30.0; 53];
        let state = crate::world_state::DeviceState::default();
        
        // Add some history
        field.add_snapshot(&sensors, &state);
        
        let temp = field.estimate_temperature(1.0, 0.0, 0.0, &sensors, &state);
        
        // Should return something reasonable (not NaN, not extreme)
        assert!(temp.is_finite());
        assert!(temp.abs() < 200.0);
    }
}
