# Async & Startup Optimization - Implementation Plan

## Critical Blocking Operations to Fix

### 1. WiFi SSID Polling - Currently Blocks UI ❌

**Current Problem:**
```rust
fn get_current_network_ssid() -> Option<String> {
    // Spawns process, waits for completion: 50-200ms!
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "active,ssid", "dev", "wifi"])
        .output();  // ← BLOCKS HERE
}
```

**Impact:**
- 50-200ms blocking UI thread
- Process spawn overhead
- No caching
- Runs every time location check happens

**Solution 1: D-Bus via zbus (Recommended)**
```rust
use zbus::blocking::Connection;
use std::sync::Arc;
use tokio::runtime::Runtime;

struct NetworkMonitor {
    ssid: Arc<Mutex<Option<String>>>,
    runtime: Runtime,
}

impl NetworkMonitor {
    fn new() -> Self {
        let ssid = Arc::new(Mutex::new(None));
        let runtime = Runtime::new().unwrap();
        
        // Start background monitor
        let ssid_clone = ssid.clone();
        runtime.spawn(async move {
            Self::monitor_network(ssid_clone).await;
        });
        
        Self { ssid, runtime }
    }
    
    async fn monitor_network(ssid: Arc<Mutex<Option<String>>>) {
        let conn = Connection::system().await.unwrap();
        
        // Query NetworkManager via D-Bus
        let proxy = NetworkManagerProxy::new(&conn).await.unwrap();
        
        loop {
            if let Ok(active_conn) = proxy.active_connection().await {
                if let Ok(new_ssid) = active_conn.ssid().await {
                    *ssid.lock().unwrap() = Some(new_ssid);
                }
            }
            
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
    
    fn get_current_ssid(&self) -> Option<String> {
        self.ssid.lock().unwrap().clone()  // Instant!
    }
}
```

**Benefits:**
- <1ms to get cached SSID
- Background updates every 5s
- No process spawn
- Real-time updates via D-Bus signals

**Solution 2: Async Process Spawn**
```rust
use tokio::process::Command;

async fn get_current_network_ssid_async() -> Option<String> {
    let output = Command::new("nmcli")
        .args(["-t", "-f", "active,ssid", "dev", "wifi"])
        .output()
        .await
        .ok()?;
    
    // Parse output...
}

// In main:
let ssid_future = tokio::spawn(async {
    get_current_network_ssid_async().await
});

// UI stays responsive while waiting
```

### 2. Model Loading - Blocks Startup ❌

**Current Problem:**
```rust
// Loads synchronously on main thread: 200-500ms!
let model = Model::load(&path)?;
```

**Solution: Lazy + Async Loading**
```rust
use tokio::fs;
use serde_json;

struct LazyModel {
    path: PathBuf,
    model: Arc<Mutex<Option<Model>>>,
    load_future: Option<JoinHandle<()>>,
}

impl LazyModel {
    fn new(path: PathBuf) -> Self {
        let model = Arc::new(Mutex::new(None));
        let model_clone = model.clone();
        let path_clone = path.clone();
        
        // Start loading in background
        let load_future = tokio::spawn(async move {
            if let Ok(contents) = fs::read_to_string(&path_clone).await {
                if let Ok(loaded) = serde_json::from_str(&contents) {
                    *model_clone.lock().unwrap() = Some(loaded);
                    println!("✓ Model loaded in background");
                }
            }
        });
        
        Self {
            path,
            model,
            load_future: Some(load_future),
        }
    }
    
    fn get_or_wait(&self) -> Arc<Mutex<Model>> {
        // Check if loaded
        if let Some(m) = self.model.lock().unwrap().as_ref() {
            return Arc::new(Mutex::new(m.clone()));
        }
        
        // Wait for background load
        if let Some(future) = &self.load_future {
            let _ = future.wait();
        }
        
        // Return loaded or default
        Arc::new(Mutex::new(
            self.model.lock().unwrap()
                .clone()
                .unwrap_or_else(|| Model::new(vec![]))
        ))
    }
}

// Usage:
println!("[3/10] Starting model load...");
let model_loader = LazyModel::new(model_path);
// Window shows immediately!

// Later, when actually needed:
let model = model_loader.get_or_wait();
```

### 3. Location Detection - Blocks Startup ❌

**Current Problem:**
```rust
let detected_idx = detect_current_location(&locations.lock().unwrap());
// Blocks while scanning WiFi networks
```

**Solution: Background Detection**
```rust
println!("[1/10] Loading locations...");
let locations = Arc::new(Mutex::new(load_locations()));
let current_location_idx = Arc::new(Mutex::new(0));  // Default

// Detect in background
let locations_clone = locations.clone();
let idx_clone = current_location_idx.clone();

std::thread::spawn(move || {
    println!("Background: Detecting location...");
    if let Some(idx) = detect_current_location(&locations_clone.lock().unwrap()) {
        *idx_clone.lock().unwrap() = idx;
        println!("✓ Location detected: {}", idx);
    }
});

// UI continues immediately
```

### 4. Database Loading - Blocks Startup ❌

**Current Problem:**
```rust
let db = Database::open(&db_path)?;  // Blocks 100-300ms
```

**Solution: Async DB with Connection Pool**
```rust
use sqlx::sqlite::SqlitePoolOptions;

async fn create_db_pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_path)
        .await
        .unwrap()
}

// Or with rusqlite async:
use tokio::task;

let db_future = task::spawn_blocking(move || {
    Database::open(&db_path)
});

// UI shows immediately
// Database loads in background
```

## SIMD Optimizations for Model Math

### Current: Scalar Operations
```rust
let prediction = self.stats
    .iter()
    .zip(readings)
    .map(|(stat, val)| {
        let normalized = (val - stat.mean) / stat.variance.sqrt();
        stat.weight * normalized
    })
    .sum::<f32>();
```

### Optimized: SIMD via packed_simd
```rust
use packed_simd::f32x8;

pub fn predict_simd(&self, readings: &[f32]) -> f32 {
    let mut sum = 0.0;
    
    // Process 8 sensors at a time
    let chunks = readings.len() / 8;
    
    for i in 0..chunks {
        let offset = i * 8;
        
        // Load 8 readings at once
        let vals = f32x8::from_slice_unaligned(&readings[offset..]);
        let means = f32x8::from_slice_unaligned(&self.means[offset..]);
        let variances = f32x8::from_slice_unaligned(&self.variances[offset..]);
        let weights = f32x8::from_slice_unaligned(&self.weights[offset..]);
        
        // Vectorized operations (8 operations in parallel!)
        let normalized = (vals - means) / variances.sqrt();
        let weighted = normalized * weights;
        
        // Sum across vector
        sum += weighted.sum();
    }
    
    // Handle remainder
    for i in (chunks * 8)..readings.len() {
        // Scalar fallback
    }
    
    sum + self.bias
}
```

**Performance:**
- 8x throughput (8 sensors in parallel)
- ~3-5x real-world speedup
- Zero allocations

## GPU Compute via wgpu (Optional, for large models)

**When to use:**
- 100+ sensors
- Complex model updates
- Real-time visualization

**Implementation:**
```rust
use wgpu;

struct GpuModel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    prediction_pipeline: wgpu::ComputePipeline,
}

impl GpuModel {
    async fn predict_gpu(&self, readings: &[f32]) -> f32 {
        // Upload sensor data to GPU
        let input_buffer = self.device.create_buffer_init(&readings);
        
        // Run compute shader
        let mut encoder = self.device.create_command_encoder();
        {
            let mut compute_pass = encoder.begin_compute_pass();
            compute_pass.set_pipeline(&self.prediction_pipeline);
            compute_pass.set_bind_group(0, &bind_group);
            compute_pass.dispatch(workgroups, 1, 1);
        }
        
        // Read result back
        self.queue.submit([encoder.finish()]);
        let result = read_buffer(&output_buffer).await;
        
        result
    }
}
```

**Compute Shader (WGSL):**
```wgsl
@group(0) @binding(0)
var<storage, read> readings: array<f32>;

@group(0) @binding(1)
var<storage, read> weights: array<f32>;

@group(0) @binding(2)
var<storage, read_write> output: f32;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    
    // Each thread processes one sensor
    let weighted = readings[idx] * weights[idx];
    
    // Atomic add to output
    atomicAdd(&output, weighted);
}
```

## Startup Optimization Strategy

### Phase 1: Immediate Window (Target: <100ms)

```rust
fn build_ui(app: &Application) {
    println!("[1/5] Building UI shell...");
    
    // Create window immediately
    let window = ApplicationWindow::new(app);
    let loading_overlay = create_loading_spinner();
    window.set_child(Some(&loading_overlay));
    
    window.present();  // ← User sees window NOW
    println!("✓ Window visible (100ms)");
    
    // Everything else happens in background
    start_background_loading(window, loading_overlay);
}
```

### Phase 2: Progressive Loading

```rust
async fn start_background_loading(window: ApplicationWindow, overlay: Overlay) {
    println!("[2/5] Loading locations...");
    let locations = tokio::spawn(async {
        load_locations().await
    });
    
    println!("[3/5] Loading model...");
    let model = tokio::spawn(async {
        Model::load_async(&model_path).await
    });
    
    println!("[4/5] Loading database...");
    let db = tokio::spawn(async {
        Database::open_async(&db_path).await
    });
    
    // Wait for all (parallel loading!)
    let (locs, mdl, database) = tokio::join!(
        locations,
        model,
        db
    );
    
    println!("[5/5] Activating UI...");
    overlay.hide();
    activate_full_ui(window, locs, mdl, database);
    println!("✓ Fully functional!");
}
```

### Phase 3: Lazy Tab Loading

```rust
// Don't load history until tab clicked
stack.connect_visible_child_name_notify(move |stack| {
    let page_name = stack.visible_child_name().unwrap();
    
    match page_name.as_str() {
        "history" => {
            if !history_loaded.get() {
                println!("Loading history on demand...");
                load_history_data();
                history_loaded.set(true);
            }
        }
        "model" => {
            if !model_viz_loaded.get() {
                println!("Generating model visualization...");
                generate_model_chart();
                model_viz_loaded.set(true);
            }
        }
        _ => {}
    }
});
```

## Dependencies to Add

```toml
[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }

# D-Bus for NetworkManager
zbus = "4"

# SIMD (optional, for model optimization)
packed_simd = "0.3"

# Async SQLite (optional)
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio"] }

# GPU compute (optional, for large models)
wgpu = { version = "0.18", optional = true }
```

## Performance Targets

| Operation | Before | After | Method |
|-----------|--------|-------|--------|
| Startup to window | 1000ms | 100ms | Lazy loading |
| WiFi SSID check | 50-200ms | <1ms | D-Bus cache |
| Model load | 200-500ms | Background | Async |
| DB open | 100-300ms | Background | Async |
| Model prediction (SIMD) | 1.2ms | 0.4ms | 8-wide SIMD |
| Model prediction (GPU) | 1.2ms | 0.1ms | Parallel compute |

## Implementation Priority

### Phase 1: Quick Wins (30 min)
1. ✅ Background location detection
2. ✅ Lazy model loading
3. ✅ Show window immediately

### Phase 2: Async I/O (1 hour)
4. ⏳ Tokio runtime integration
5. ⏳ Async file loading
6. ⏳ Async database

### Phase 3: D-Bus WiFi (45 min)
7. ⏳ zbus integration
8. ⏳ NetworkManager proxy
9. ⏳ Background SSID monitor

### Phase 4: SIMD (Optional, 2 hours)
10. ⏳ packed_simd integration
11. ⏳ Vectorized model math
12. ⏳ Benchmarks

### Phase 5: GPU (Optional, 4+ hours)
13. ⏳ wgpu setup
14. ⏳ Compute shaders
15. ⏳ Fallback logic

Let's implement these improvements!
