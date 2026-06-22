# Code Architecture Analysis & Improvement Plan

## 🎯 **Current Architecture Overview**

### File Structure
```
src/
├── main.rs              (5,400+ lines) ⚠️ MONOLITHIC
├── helpers.rs           (1,100+ lines)
├── temperature_field.rs (1,040 lines)
├── forecast.rs          (255 lines)
├── sync.rs              (460 lines)
├── security.rs          (~300 lines)
├── dbmaint.rs           (~200 lines)
├── network_viz.rs       (~150 lines)
├── sphere.rs            (367 lines)
└── style.css            (CSS styling)
```

**Problem:** `main.rs` contains:
- UI layout (GTK4-specific)
- Business logic
- Database operations
- Sensor polling
- Event handlers
- Model training
- All mixed together!

---

## 🏗️ **Proposed Architecture: MVC + Repository Pattern**

### Goal
- **Decouple UI from business logic**
- **Enable easy UI framework swapping** (GTK4 → Iced → Qt → Slint)
- **Improve testability**
- **Better code organization**

### New Structure

```
src/
├── main.rs                    // Entry point only (50 lines)
├── app.rs                     // Application controller
│
├── ui/                        // UI Layer (Framework-specific)
│   ├── mod.rs
│   ├── gtk4/                  // Current GTK4 implementation
│   │   ├── mod.rs
│   │   ├── pages/
│   │   │   ├── live.rs
│   │   │   ├── model.rs
│   │   │   ├── history.rs
│   │   │   ├── locations.rs
│   │   │   └── sync.rs
│   │   ├── widgets/
│   │   │   ├── temperature_display.rs
│   │   │   └── calibration_dialog.rs
│   │   └── theme.rs
│   ├── iced/                  // Future: Iced implementation
│   └── qt/                    // Future: Qt implementation
│
├── core/                      // Business Logic (UI-agnostic)
│   ├── mod.rs
│   ├── models/
│   │   ├── linear_model.rs   // Current Model struct
│   │   ├── neural_field.rs   // Current TemperatureField
│   │   └── forecast.rs
│   ├── sensors/
│   │   ├── thermal_poller.rs
│   │   └── device_state.rs
│   └── calibration.rs
│
├── data/                      // Data Layer
│   ├── mod.rs
│   ├── database.rs           // Database operations
│   ├── repositories/
│   │   ├── calibration_repo.rs
│   │   ├── reading_repo.rs
│   │   └── location_repo.rs
│   └── models/               // Data models
│       ├── calibration.rs
│       ├── reading.rs
│       └── location.rs
│
├── services/                  // Application Services
│   ├── mod.rs
│   ├── prediction_service.rs // Wraps model prediction
│   ├── sync_service.rs
│   └── compression_service.rs
│
└── utils/
    ├── helpers.rs
    ├── compression.rs
    └── validation.rs
```

---

## 📋 **Refactoring Roadmap**

### Phase 1: Extract Business Logic (Week 1)

**Goal:** Move core logic out of main.rs

#### Step 1.1: Extract Model Layer
```rust
// src/core/models/linear_model.rs
pub struct LinearModel {
    // Current Model struct
}

impl LinearModel {
    pub fn new(sensor_names: Vec<String>) -> Self { ... }
    pub fn calibrate(&mut self, sensors: &[f32], target: f32) { ... }
    pub fn predict(&self, sensors: &[f32]) -> f32 { ... }
    pub fn save(&self, path: &Path) -> Result<(), String> { ... }
    pub fn load(path: &Path) -> Result<Self, String> { ... }
}
```

#### Step 1.2: Extract Sensor Polling
```rust
// src/core/sensors/thermal_poller.rs
pub struct ThermalPoller {
    // Current implementation
}

impl ThermalPoller {
    pub fn new() -> Self { ... }
    pub fn poll(&mut self) -> Option<Vec<f32>> { ... }
    pub fn sensor_names(&self) -> Vec<String> { ... }
}
```

#### Step 1.3: Extract Calibration Service
```rust
// src/services/calibration_service.rs
pub struct CalibrationService {
    linear_model: Arc<Mutex<LinearModel>>,
    neural_field: Arc<Mutex<TemperatureField>>,
    database: Arc<Mutex<Database>>,
}

impl CalibrationService {
    pub async fn calibrate(&self, target: f32) -> Result<(), String> {
        // 1. Collect burst readings
        // 2. Average readings
        // 3. Update models
        // 4. Save models
        // 5. Store in database
        // All in one place!
    }
}
```

---

### Phase 2: Extract Data Layer (Week 2)

**Goal:** Separate database operations from business logic

#### Repository Pattern
```rust
// src/data/repositories/calibration_repo.rs
pub struct CalibrationRepository {
    db: Arc<Mutex<Database>>,
}

impl CalibrationRepository {
    pub fn save(&self, calibration: &Calibration) -> Result<(), Error> { ... }
    pub fn get_all(&self) -> Result<Vec<Calibration>, Error> { ... }
    pub fn get_by_location(&self, location_id: &str) -> Result<Vec<Calibration>, Error> { ... }
    pub fn count(&self) -> Result<usize, Error> { ... }
}
```

**Benefits:**
- Easy to swap SQLite for PostgreSQL
- Easy to add caching layer
- Easy to test (mock repository)

---

### Phase 3: Create UI Abstraction (Week 3)

**Goal:** Define UI-agnostic traits

#### UI Trait
```rust
// src/ui/mod.rs
pub trait UI {
    fn run(self, app: Application) -> Result<(), String>;
}

pub struct Application {
    pub calibration_service: Arc<CalibrationService>,
    pub prediction_service: Arc<PredictionService>,
    pub sync_service: Arc<SyncService>,
    // ... other services
}

// Events from UI to Core
pub enum UIEvent {
    Calibrate(f32),
    LogWeather(f32, String),
    SwitchLocation(usize),
    Bootstrap(PathBuf),
}

// State updates from Core to UI
pub enum StateUpdate {
    Temperature(f32, f32),  // (temp, confidence)
    ModelType(String),
    CalibrationCount(usize),
    Error(String),
}
```

---

### Phase 4: Implement GTK4 UI (Week 4)

**Goal:** Move current GTK4 code into modular structure

```rust
// src/ui/gtk4/mod.rs
pub struct GTK4UI;

impl UI for GTK4UI {
    fn run(self, app: Application) -> Result<(), String> {
        // GTK4-specific setup
        let gtk_app = gtk::Application::new(...);
        
        // Create pages
        let pages = Pages::new(&app);
        
        // Wire events
        gtk_app.run()
    }
}

// src/ui/gtk4/pages/live.rs
pub struct LivePage {
    calibration_service: Arc<CalibrationService>,
}

impl LivePage {
    pub fn build(&self) -> gtk::Box {
        // Current Page 1 code
    }
    
    fn on_calibrate(&self, target: f32) {
        // Call service, not inline logic
        self.calibration_service.calibrate(target);
    }
}
```

---

## 🔄 **Benefits of New Architecture**

### 1. Easy UI Framework Swap

**Current (GTK4):**
```rust
// main.rs
fn main() {
    let ui = GTK4UI;
    let app = Application::new();
    ui.run(app);
}
```

**Switch to Iced:**
```rust
// main.rs
fn main() {
    let ui = IcedUI;  // ← Only this changes!
    let app = Application::new();
    ui.run(app);
}
```

---

### 2. Testability

**Before:**
```rust
// Can't test - tightly coupled to GTK
calibrate_btn.connect_clicked(move |_| {
    // 200 lines of logic
});
```

**After:**
```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_calibration() {
        let service = CalibrationService::new(...);
        let result = service.calibrate(20.0);
        assert!(result.is_ok());
    }
}
```

---

### 3. Code Reuse

**Example:** Command-line tool
```rust
// src/bin/calibrate.rs
fn main() {
    let app = Application::new();
    app.calibration_service.calibrate(20.0).unwrap();
    println!("Calibrated!");
}
```

Same business logic, no UI!

---

## 🎨 **UI Framework Comparison**

### GTK4 (Current)
**Pros:**
- ✅ Native Linux integration
- ✅ Good documentation
- ✅ Mature ecosystem
- ✅ Libadwaita for modern design

**Cons:**
- ❌ Linux-focused (Windows/macOS support exists but secondary)
- ❌ Verbose code
- ❌ GObject boilerplate

---

### Iced (Rust-native)
**Pros:**
- ✅ Pure Rust
- ✅ Cross-platform (Windows, Linux, macOS)
- ✅ Elm-inspired architecture (very clean)
- ✅ Good performance

**Cons:**
- ❌ Smaller ecosystem
- ❌ Less mature than GTK
- ❌ Fewer pre-built widgets

**Example:**
```rust
enum Message {
    Calibrate(f32),
    TemperatureUpdated(f32),
}

impl Application for App {
    fn update(&mut self, message: Message) {
        match message {
            Message::Calibrate(temp) => {
                self.calibration_service.calibrate(temp);
            }
            Message::TemperatureUpdated(temp) => {
                self.current_temp = temp;
            }
        }
    }
}
```

---

### Slint (Declarative)
**Pros:**
- ✅ Declarative UI (like QML)
- ✅ Very clean separation
- ✅ Good performance
- ✅ Cross-platform

**Cons:**
- ❌ New/immature
- ❌ Learning curve (new language)

**Example:**
```slint
export component MainWindow {
    in property <float> temperature;
    callback calibrate(float);
    
    Text {
        text: "Temperature: \{temperature}°C";
    }
    
    Button {
        text: "Calibrate";
        clicked => { calibrate(20.0); }
    }
}
```

---

### Qt (C++ with Rust bindings)
**Pros:**
- ✅ Very mature
- ✅ Huge widget library
- ✅ Cross-platform
- ✅ Professional applications

**Cons:**
- ❌ C++ baggage
- ❌ Rust bindings less mature
- ❌ Licensing complexity (GPL/LGPL/Commercial)

---

## 📊 **Current Code Issues & Fixes**

### Issue 1: Massive main.rs (5,400 lines)

**Current:**
```rust
// main.rs
fn main() {
    // 200 lines: Sensor setup
    // 500 lines: Database setup
    // 1000 lines: UI layout
    // 2000 lines: Event handlers
    // 500 lines: Model loading
    // ... everything!
}
```

**Fix:**
```rust
// main.rs (50 lines)
fn main() {
    let app = Application::new();
    let ui = GTK4UI::new();
    ui.run(app).unwrap();
}

// app.rs
impl Application {
    pub fn new() -> Self {
        let sensors = SensorService::new();
        let database = DatabaseService::new();
        let calibration = CalibrationService::new(...);
        // ... compose services
    }
}
```

---

### Issue 2: Tight Coupling

**Current:**
```rust
// GTK button directly calls model
calibrate_btn.connect_clicked(move |_| {
    let mut model = model.lock().unwrap();
    model.calibrate(...);  // Tight coupling!
});
```

**Fix:**
```rust
// GTK button emits event
calibrate_btn.connect_clicked(move |_| {
    event_tx.send(UIEvent::Calibrate(20.0));
});

// Service handles event
match event {
    UIEvent::Calibrate(temp) => {
        self.calibration_service.calibrate(temp);
    }
}
```

---

### Issue 3: No Error Handling Strategy

**Current:**
```rust
let result = model.save(&path);  // Returns Result
// ... but then what? Mixed .unwrap(), .ok(), .unwrap_or()
```

**Fix:**
```rust
// Define error types
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    
    #[error("Model error: {0}")]
    Model(String),
    
    #[error("Sensor error: {0}")]
    Sensor(String),
}

// Consistent error handling
pub type Result<T> = std::result::Result<T, AppError>;
```

---

### Issue 4: Global State (Arc<Mutex<T>> everywhere)

**Current:**
```rust
let model = Arc::new(Mutex::new(...));
let db = Arc::new(Mutex::new(...));
let neural = Arc::new(Mutex::new(...));
// ... clone everywhere!
```

**Fix:**
```rust
// State management
pub struct AppState {
    pub model: LinearModel,
    pub neural_field: TemperatureField,
    pub database: Database,
}

// Single Arc<Mutex<AppState>>
let state = Arc::new(Mutex::new(AppState::new()));
```

Or use **channels** for state updates:
```rust
// Update channel
let (state_tx, state_rx) = mpsc::channel();

// Service updates state
state_tx.send(StateUpdate::Temperature(14.2, 0.85));

// UI receives updates
for update in state_rx {
    match update {
        StateUpdate::Temperature(t, c) => {
            temp_label.set_text(&format!("{:.1}°C", t));
        }
    }
}
```

---

## 🚀 **Implementation Priority**

### Immediate (This Week)
1. ✅ Extract `LinearModel` to `src/core/models/linear_model.rs`
2. ✅ Extract `TemperatureField` (already separate!)
3. ✅ Extract `ThermalPoller` to `src/core/sensors/`
4. ✅ Create `CalibrationService`

### Short Term (Next 2 Weeks)
5. ⬜ Create repository pattern for database
6. ⬜ Extract UI pages to separate modules
7. ⬜ Define `UI` trait
8. ⬜ Refactor main.rs to <100 lines

### Medium Term (Month 2)
9. ⬜ Implement Iced UI as alternative
10. ⬜ Add proper error handling
11. ⬜ Add comprehensive tests
12. ⬜ Create CLI tool using same core

### Long Term (Month 3+)
13. ⬜ Evaluate Slint for declarative UI
14. ⬜ Create plugin system for models
15. ⬜ Add web UI (WASM + Yew/Leptos)

---

## 📁 **File Size Recommendations**

**Current:**
```
main.rs:              5,400 lines  ⚠️ WAY TOO BIG
helpers.rs:           1,100 lines  ⚠️ TOO BIG
temperature_field.rs: 1,040 lines  ⚠️ TOO BIG
```

**Target:**
```
main.rs:              <100 lines   ✓ Entry point only
app.rs:               <300 lines   ✓ Application setup
services/*.rs:        <500 lines   ✓ Per service
ui/pages/*.rs:        <400 lines   ✓ Per page
core/models/*.rs:     <600 lines   ✓ Per model
```

**Rule of thumb:** If a file is >1000 lines, it's doing too much!

---

## 🧪 **Testing Strategy**

### Unit Tests (Business Logic)
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn linear_model_calibration() {
        let mut model = LinearModel::new(vec![...]);
        model.calibrate(&[30.0, 35.0], 20.0);
        assert!(model.predict(&[30.0, 35.0]) - 20.0 < 1.0);
    }
}
```

### Integration Tests (Services)
```rust
#[test]
fn calibration_service_full_flow() {
    let service = CalibrationService::new_test();
    service.calibrate(20.0).unwrap();
    assert_eq!(service.get_calibration_count(), 1);
}
```

### UI Tests (Framework-specific)
```rust
#[gtk::test]
fn calibrate_button_triggers_service() {
    let service = Arc::new(MockCalibrationService::new());
    let page = LivePage::new(service.clone());
    
    page.calibrate_button.emit_clicked();
    
    assert!(service.was_called());
}
```

---

## 🎯 **Next Steps: Quick Wins**

### 1. Extract CalibrationService (1 hour)
Move calibration logic from button handler to dedicated service.

### 2. Extract PredictionService (1 hour)
Centralize all prediction logic (linear, neural, forecasts).

### 3. Split main.rs into app.rs (2 hours)
Move application setup out of main.

### 4. Create ui/gtk4/ module structure (3 hours)
Organize GTK code by page/widget.

**Total:** ~7 hours for massive improvement in code quality!

---

## 💡 **Summary**

**Current State:**
- ❌ Monolithic main.rs (5,400 lines)
- ❌ Tight coupling (UI ↔ Business Logic ↔ Data)
- ❌ Hard to test
- ❌ Can't swap UI framework

**Target State:**
- ✅ Modular architecture (files <600 lines)
- ✅ Clear separation (UI ← Services → Data)
- ✅ Testable (unit + integration tests)
- ✅ Framework-agnostic core (swap GTK4 ↔ Iced ↔ Slint)

**Path Forward:**
1. Extract services (business logic)
2. Create repository pattern (data access)
3. Define UI trait (abstraction layer)
4. Implement modular GTK4 UI
5. Add alternative UI (Iced or Slint)

**The code will be:**
- **More maintainable** (easier to find/fix bugs)
- **More testable** (unit tests for everything)
- **More flexible** (easy to add features)
- **Future-proof** (can swap UI frameworks)

Let's start refactoring! 🚀
