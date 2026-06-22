# Performance & Responsiveness Optimization Plan

## Critical Issues to Fix

### 1. Database Locking UI ⚠️ URGENT
**Problem:** Database writes block UI thread
**Impact:** UI freezes during saves, especially on slow mobile storage

**Solutions:**
- Move ALL database writes to background thread pool
- Use mpsc channel for write queue
- Batch writes together
- Never call db.lock() from UI callbacks

### 2. Slow Startup ⚠️
**Problem:** Loading model/database on main thread
**Current:** 2-3 seconds
**Target:** <500ms to window show

**Solutions:**
- Show window FIRST with loading indicator
- Load model/database in background
- Progressive UI updates as data loads
- Lazy load history tab

### 3. Separate Live vs Historical Data ✅
**Problem:** Live thermometer mixes with historical logs

**New Structure:**
```rust
// Live thermometer - real-time display only
struct LiveReading {
    sensors: Vec<f32>,
    prediction: f32,
    timestamp: Instant,  // In-memory only
}

// Historical data point - logged to database
struct HistoricalReading {
    sensors: Vec<f32>,
    actual_temp: f32,    // From calibration
    predicted_temp: f32,
    location_id: String,
    timestamp: i64,      // Unix timestamp
}
```

**Button Actions:**
- Live display: Updates every 100ms, never saved
- Calibrate: Logs calibration + creates historical point
- Manual "Log to History": Saves current reading as datapoint

## Optimization Roadmap

### Phase 1: Database Thread Pool (30 min)

```rust
use crossbeam_channel::{bounded, Sender, Receiver};
use std::thread;

struct DatabaseWriter {
    write_tx: Sender<DbOperation>,
}

enum DbOperation {
    StoreReading(SensorReading, Option<String>),
    StoreCalibration(Vec<f32>, f32, Option<String>),
    StoreHistoricalPoint(HistoricalReading),
}

impl DatabaseWriter {
    fn new(db: Arc<Mutex<Database>>) -> Self {
        let (tx, rx) = bounded(100); // Buffer up to 100 operations
        
        // Background writer thread
        thread::spawn(move || {
            while let Ok(op) = rx.recv() {
                match op {
                    DbOperation::StoreReading(reading, loc) => {
                        if let Ok(db) = db.lock() {
                            let _ = db.store_reading(&reading, loc.as_deref());
                        }
                    }
                    DbOperation::StoreCalibration(sensors, target, loc) => {
                        if let Ok(db) = db.lock() {
                            let _ = db.store_calibration(&sensors, target, loc.as_deref());
                        }
                    }
                    DbOperation::StoreHistoricalPoint(point) => {
                        if let Ok(db) = db.lock() {
                            let _ = db.store_historical_point(&point);
                        }
                    }
                }
            }
        });
        
        Self { write_tx: tx }
    }
    
    fn log_calibration(&self, sensors: Vec<f32>, target: f32, location_id: Option<String>) {
        // Non-blocking send
        let _ = self.write_tx.try_send(DbOperation::StoreCalibration(sensors, target, location_id));
    }
}
```

**Benefits:**
- UI thread NEVER blocks on database
- Automatic batching via channel buffer
- Graceful degradation if buffer full (skip vs freeze)

### Phase 2: Lazy Loading (20 min)

```rust
// Don't load model/database until needed
struct LazyLoad<T> {
    data: Arc<Mutex<Option<T>>>,
    loader: Arc<Mutex<Option<Box<dyn FnOnce() -> T + Send>>>>,
}

impl<T: Send + 'static> LazyLoad<T> {
    fn new<F>(loader: F) -> Self 
    where F: FnOnce() -> T + Send + 'static
    {
        Self {
            data: Arc::new(Mutex::new(None)),
            loader: Arc::new(Mutex::new(Some(Box::new(loader)))),
        }
    }
    
    fn get_or_load(&self) -> Arc<Mutex<T>> {
        let mut data = self.data.lock().unwrap();
        if data.is_none() {
            let mut loader = self.loader.lock().unwrap();
            if let Some(load_fn) = loader.take() {
                *data = Some(load_fn());
            }
        }
        
        // Return Arc to the loaded data
        Arc::new(Mutex::new(data.as_ref().unwrap().clone()))
    }
}

// Usage:
let model = LazyLoad::new(|| {
    // Heavy loading happens here, not at startup
    Model::load_latest().unwrap_or_else(|| Model::new())
});

// Window shows immediately, model loads on first access
```

### Phase 3: Separate Live/Historical (45 min)

```rust
// Add to database schema
CREATE TABLE IF NOT EXISTS historical_points (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    location_id TEXT NOT NULL,
    actual_temp REAL NOT NULL,      -- From user calibration
    predicted_temp REAL NOT NULL,   -- What model predicted
    sensors TEXT NOT NULL,          -- JSON array of sensor readings
    confidence REAL,                -- Model confidence
    notes TEXT                      -- Optional user notes
);

// New UI button on live page
let log_to_history_btn = gtk::Button::with_label("📝 Log to History");

log_to_history_btn.connect_clicked(move |_| {
    // Show dialog to enter actual temperature
    let dialog = adw::MessageDialog::new(
        &window,
        "Log Current Reading",
        "Enter the actual temperature to log this reading to history"
    );
    
    // ... on confirm:
    let point = HistoricalReading {
        timestamp: current_timestamp(),
        location_id: current_location.id.clone(),
        actual_temp: user_entered_temp,
        predicted_temp: current_prediction,
        sensors: current_sensor_readings.clone(),
        confidence: model.get_confidence(),
        notes: None,
    };
    
    db_writer.log_historical_point(point);  // Non-blocking!
});
```

### Phase 4: UI Responsiveness (30 min)

**Problem:** Lock contention during UI updates

**Solution: Read-only cache**
```rust
struct ModelCache {
    last_prediction: Arc<Mutex<f32>>,
    last_confidence: Arc<Mutex<f32>>,
    last_update: Arc<Mutex<Instant>>,
}

impl ModelCache {
    fn update_from_model(&self, model: &Model) {
        *self.last_prediction.lock().unwrap() = model.last_prediction;
        *self.last_confidence.lock().unwrap() = model.get_confidence();
        *self.last_update.lock().unwrap() = Instant::now();
    }
    
    fn get_prediction(&self) -> f32 {
        *self.last_prediction.lock().unwrap()
    }
}

// UI update loop reads from cache, not model
glib::timeout_add_local(Duration::from_millis(100), move || {
    let prediction = model_cache.get_prediction();  // Fast!
    temp_label.set_text(&format!("{:.1}°C", prediction));
    glib::ControlFlow::Continue
});

// Model update happens separately
thread::spawn(move || {
    loop {
        thread::sleep(Duration::from_millis(500));
        
        if let Ok(mut model) = model.try_lock() {
            // Update model
            model.predict(&sensor_readings);
            
            // Update cache for UI
            model_cache.update_from_model(&model);
        }
        // If lock busy, skip this cycle (UI still responsive!)
    }
});
```

### Phase 5: Startup Optimization (20 min)

**Current flow:**
```
Load model (500ms) → Load DB (300ms) → Build UI (200ms) → Show window
= 1000ms until visible
```

**Optimized flow:**
```
Build UI (100ms) → Show window with "Loading..." → Background load
= 100ms until visible, 1000ms until functional
```

```rust
fn build_ui(app: &adw::Application) {
    println!("[1/3] Building UI...");
    
    // Build UI with loading state
    let window = create_window(app);
    let loading_spinner = create_loading_overlay();
    
    window.present();  // Show immediately!
    println!("✓ Window visible in 100ms");
    
    // Load heavy data in background
    let (tx, rx) = glib::MainContext::channel(glib::Priority::DEFAULT);
    
    thread::spawn(move || {
        println!("[2/3] Loading model...");
        let model = load_model();  // 500ms
        
        println!("[3/3] Loading database...");
        let db = load_database();  // 300ms
        
        // Send to UI thread
        tx.send((model, db)).unwrap();
    });
    
    // Receive and activate
    rx.attach(None, move |(model, db)| {
        loading_spinner.hide();
        activate_ui(model, db);
        println!("✓ Fully functional!");
        glib::ControlFlow::Break
    });
}
```

### Phase 6: Micro-optimizations (15 min)

```rust
// 1. Pre-allocate strings
struct StringBuffer {
    temp_buf: String,
    time_buf: String,
}

impl StringBuffer {
    fn format_temp(&mut self, temp: f32) -> &str {
        use std::fmt::Write;
        self.temp_buf.clear();
        write!(&mut self.temp_buf, "{:.1}°C", temp).unwrap();
        &self.temp_buf
    }
}

// 2. Reduce allocations in hot path
fn predict(&mut self, sensors: &[f32]) -> f32 {
    // Before: Allocates vector
    // let weighted: Vec<f32> = sensors.iter()
    //     .zip(&self.stats)
    //     .map(|(s, stat)| s * stat.weight)
    //     .collect();
    
    // After: Direct sum, no allocation
    sensors.iter()
        .zip(&self.stats)
        .map(|(s, stat)| s * stat.weight)
        .sum::<f32>() + self.bias
}

// 3. Cache expensive computations
struct ChartCache {
    last_svg: Arc<Mutex<Option<String>>>,
    last_data_hash: Arc<Mutex<u64>>,
}

impl ChartCache {
    fn get_or_generate(&self, data: &[DataPoint]) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut hasher);
        let hash = hasher.finish();
        
        let mut last_hash = self.last_data_hash.lock().unwrap();
        if *last_hash == hash {
            // Data unchanged, return cached SVG
            return self.last_svg.lock().unwrap().clone().unwrap();
        }
        
        // Data changed, regenerate
        let svg = generate_chart(data);
        *self.last_svg.lock().unwrap() = Some(svg.clone());
        *last_hash = hash;
        svg
    }
}
```

## Performance Targets

| Metric | Current | Target | How |
|--------|---------|--------|-----|
| Startup to window | 1000ms | 100ms | Lazy loading |
| Startup to functional | 1000ms | 1500ms | Progressive load |
| UI freeze during save | 50-200ms | 0ms | Write queue |
| Calibration response | 100ms | 50ms | Background thread |
| Chart generation | 100ms | 50ms | Caching |
| Lock contention | Common | Rare | Read cache |

## Testing

```rust
#[cfg(test)]
mod perf_tests {
    use super::*;
    use std::time::Instant;
    
    #[test]
    fn test_prediction_speed() {
        let model = Model::new(vec!["sensor1".into()]);
        let sensors = vec![45.0];
        
        let start = Instant::now();
        for _ in 0..10000 {
            model.predict(&sensors);
        }
        let elapsed = start.elapsed();
        
        // Should be <1ms for 10k predictions
        assert!(elapsed < Duration::from_millis(1));
    }
    
    #[test]
    fn test_database_write_async() {
        let db_writer = DatabaseWriter::new(db);
        
        let start = Instant::now();
        db_writer.log_calibration(vec![45.0], 22.0, None);
        let elapsed = start.elapsed();
        
        // Should return immediately (<1ms)
        assert!(elapsed < Duration::from_millis(1));
    }
}
```

## Implementation Order

1. ✅ Database write queue (biggest impact on UI freeze)
2. ✅ Model read cache (eliminates lock contention)
3. ✅ Lazy loading (fast startup)
4. ✅ Separate live/historical (clean data model)
5. ⏳ Micro-optimizations (polish)
6. ⏳ Performance tests (validation)

## Dependencies to Add

```toml
[dependencies]
# Existing...
crossbeam-channel = "0.5"  # For fast, bounded queues
```

## Expected Results

**Before:**
- Window appears: 1000ms
- UI freezes during save: 200ms
- Lock contention: Every 100ms
- Calibration feels sluggish

**After:**
- Window appears: 100ms ✨
- UI freezes: 0ms ✨
- Lock contention: Rare (use try_lock)
- Calibration feels instant ✨
- Clean separation of live vs logged data ✨

Start with Phase 1 (database queue) for immediate improvement!
