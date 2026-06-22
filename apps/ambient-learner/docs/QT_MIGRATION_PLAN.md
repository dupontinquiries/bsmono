# Qt/QML Migration Plan for Ambient Learner

## 🎯 **Why Qt/QML Makes Sense**

### Your Current Situation
- ✅ **GTK4 working** but Linux-focused
- ⚠️ **Want cross-platform** (Linux, Windows, macOS, Android)
- ⚠️ **Want 3D heat sphere** visualization
- ⚠️ **Complex sensor→ML→UI** pipeline

### Qt/QML Benefits for This Project
1. **Declarative UI** - QML is perfect for responsive, animated interfaces
2. **Qt 3D** - Built-in 3D engine for your heat sphere orb
3. **Cross-platform** - Write once, run everywhere
4. **QML + Rust** - Clean separation via `qmetaobject` crate
5. **Performance** - Hardware-accelerated rendering
6. **Mobile-ready** - Same code works on Android/iOS

---

## 🏗️ **Recommended Architecture: WorldState Pattern**

Based on the robotics pattern you referenced, this is PERFECT for your app:

```
┌─────────────────────────────────────────────────────────┐
│                     WorldState                          │
│              Arc<RwLock<SystemState>>                   │
│                                                         │
│  - sensors: Vec<f32>                                   │
│  - prediction: f32                                     │
│  - confidence: f32                                     │
│  - model_type: String                                  │
│  - calibration_count: usize                            │
│  - sphere_samples: Vec<(Point3D, f32)>                │
│  - dirty_flags: DirtyFlags                             │
└─────────────────────────────────────────────────────────┘
                    ↑↓                  ↑↓
    ┌───────────────┘                   └──────────────┐
    │                                                   │
┌───▼─────┐  ┌──────────┐  ┌──────────┐  ┌───────────▼──┐
│ Sensor  │  │  Model   │  │ Database │  │   Qt/QML     │
│ System  │  │  System  │  │  System  │  │   Backend    │
│ Thread  │  │  Thread  │  │  Thread  │  │   (UI poll)  │
└─────────┘  └──────────┘  └──────────┘  └──────────────┘
```

### Why This Beats Message Passing

**Your project has:**
- ✅ **Large mutable state** (neural field, history buffer)
- ✅ **Continuous data streams** (sensor polling)
- ✅ **Heavy computation** (ML inference)

**Message passing struggles with:**
- ❌ "Message explosion" - too many events
- ❌ Copying large state repeatedly
- ❌ Complex synchronization

**WorldState wins:**
- ✅ Single source of truth
- ✅ Read-heavy (UI polls), write-light (sensors update)
- ✅ `RwLock` allows many readers, one writer
- ✅ Dirty flags prevent unnecessary updates

---

## 📁 **New Project Structure**

```
ambient-learner-qt/
├── Cargo.toml
├── build.rs                  # qmetaobject build
├── qml/
│   ├── main.qml             # Main window
│   ├── pages/
│   │   ├── LiveView.qml     # Sensor display
│   │   ├── SphereView.qml   # 3D heat orb
│   │   ├── HistoryView.qml  # Charts
│   │   └── CalibrationView.qml
│   ├── components/
│   │   ├── TemperatureDisplay.qml
│   │   ├── CalibrationDialog.qml
│   │   └── SphereRenderer.qml
│   └── Theme.qml            # Color/font definitions
│
├── src/
│   ├── main.rs              # Entry point (50 lines)
│   ├── qt_backend.rs        # Qt↔Rust bridge
│   ├── world_state.rs       # Central state
│   │
│   ├── systems/             # Worker threads
│   │   ├── sensor_system.rs
│   │   ├── model_system.rs
│   │   ├── database_system.rs
│   │   └── sphere_system.rs
│   │
│   ├── core/               # Shared with GTK version!
│   │   ├── models/
│   │   │   ├── linear_model.rs
│   │   │   └── temperature_field.rs
│   │   ├── sensors/
│   │   │   └── thermal_poller.rs
│   │   └── services/
│   │       └── calibration.rs
│   │
│   └── data/
│       ├── database.rs
│       └── compression.rs
│
└── assets/
    └── shaders/
        └── heat_field.frag  # 3D sphere shader
```

**Key insight:** `core/` folder is **identical** between GTK and Qt versions!

---

## 💾 **WorldState Definition**

```rust
// src/world_state.rs

use std::sync::{Arc, RwLock};
use std::collections::VecDeque;
use std::time::Instant;

/// Central application state shared across all systems
pub struct SystemState {
    // Sensor data
    pub sensors: Vec<f32>,
    pub device_state: DeviceState,
    pub last_sensor_update: Instant,
    
    // Predictions
    pub ambient_temp: f32,
    pub confidence: f32,
    pub model_type: ModelType,
    
    // Calibration
    pub calibration_count: usize,
    pub linear_calibrations: usize,
    pub neural_calibrations: usize,
    
    // History
    pub sensor_history: VecDeque<(Instant, Vec<f32>)>,
    pub temperature_history: VecDeque<(Instant, f32)>,
    
    // 3D Sphere data
    pub sphere_samples: Vec<SpherePoint>,
    
    // Location
    pub current_location: String,
    pub locations: Vec<Location>,
    
    // Dirty flags
    pub flags: DirtyFlags,
}

#[derive(Default)]
pub struct DirtyFlags {
    pub sensors: bool,
    pub model: bool,
    pub ui: bool,
    pub sphere: bool,
    pub database: bool,
}

#[derive(Clone)]
pub struct SpherePoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub temperature: f32,
}

#[derive(Clone, Copy)]
pub enum ModelType {
    Linear,
    Neural,
}

pub type WorldState = Arc<RwLock<SystemState>>;

impl SystemState {
    pub fn new() -> Self {
        Self {
            sensors: Vec::new(),
            device_state: DeviceState::default(),
            last_sensor_update: Instant::now(),
            ambient_temp: 20.0,
            confidence: 0.0,
            model_type: ModelType::Linear,
            calibration_count: 0,
            linear_calibrations: 0,
            neural_calibrations: 0,
            sensor_history: VecDeque::with_capacity(300),
            temperature_history: VecDeque::with_capacity(1000),
            sphere_samples: Vec::new(),
            current_location: "Home".to_string(),
            locations: Vec::new(),
            flags: DirtyFlags::default(),
        }
    }
}
```

---

## 🔌 **Qt Backend Bridge**

```rust
// src/qt_backend.rs

use qmetaobject::*;
use std::sync::{Arc, RwLock};
use crate::world_state::{SystemState, WorldState, ModelType};

#[derive(QObject, Default)]
pub struct QtBackend {
    base: qt_base_class!(trait QObject),
    
    // Properties exposed to QML
    ambient_temp: qt_property!(f32; NOTIFY ambient_changed),
    confidence: qt_property!(f32; NOTIFY confidence_changed),
    model_type: qt_property!(QString; NOTIFY model_type_changed),
    calibration_count: qt_property!(i32; NOTIFY calibration_count_changed),
    sensor_count: qt_property!(i32; NOTIFY sensor_count_changed),
    
    // 3D sphere data (as JSON string for QML)
    sphere_data: qt_property!(QString; NOTIFY sphere_data_changed),
    
    // Signals
    ambient_changed: qt_signal!(),
    confidence_changed: qt_signal!(),
    model_type_changed: qt_signal!(),
    calibration_count_changed: qt_signal!(),
    sensor_count_changed: qt_signal!(),
    sphere_data_changed: qt_signal!(),
    
    // Internal state reference
    world_state: Option<WorldState>,
}

impl QtBackend {
    /// Attach the world state
    pub fn attach_state(&mut self, state: WorldState) {
        self.world_state = Some(state);
    }
    
    /// Poll for updates (called by QML timer)
    fn poll_updates(&mut self) {
        if let Some(ref state) = self.world_state {
            let mut world = state.write().unwrap();
            
            if world.flags.ui {
                // Update properties
                self.ambient_temp = world.ambient_temp;
                self.confidence = world.confidence;
                self.model_type = match world.model_type {
                    ModelType::Linear => "Linear".into(),
                    ModelType::Neural => "Neural".into(),
                };
                self.calibration_count = world.calibration_count as i32;
                self.sensor_count = world.sensors.len() as i32;
                
                // Emit signals to update QML
                self.ambient_changed();
                self.confidence_changed();
                self.model_type_changed();
                self.calibration_count_changed();
                self.sensor_count_changed();
                
                world.flags.ui = false;
            }
            
            if world.flags.sphere {
                // Serialize sphere data for QML
                let json = serde_json::to_string(&world.sphere_samples)
                    .unwrap_or_default();
                self.sphere_data = json.into();
                self.sphere_data_changed();
                
                world.flags.sphere = false;
            }
        }
    }
    
    /// Trigger calibration (called from QML button)
    fn calibrate(&mut self, target_temp: f32) {
        println!("🎯 Calibration requested: {}°C", target_temp);
        
        // In full implementation, this would send a message
        // to the calibration service via a channel
        
        // For now, just mark the model as needing update
        if let Some(ref state) = self.world_state {
            let mut world = state.write().unwrap();
            world.flags.model = true;
        }
    }
}
```

---

## 🧵 **Sensor System Thread**

```rust
// src/systems/sensor_system.rs

use std::sync::RwLock;
use std::time::{Duration, Instant};
use crate::world_state::WorldState;
use crate::core::sensors::ThermalPoller;

pub fn spawn_sensor_system(world: WorldState) {
    std::thread::spawn(move || {
        let mut poller = ThermalPoller::new();
        let mut last_poll = Instant::now();
        
        loop {
            // Poll every 100ms (passive interval)
            if last_poll.elapsed() < Duration::from_millis(100) {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            last_poll = Instant::now();
            
            // Read sensors
            if let Some(readings) = poller.poll() {
                let mut state = world.write().unwrap();
                
                // Update state
                state.sensors = readings;
                state.device_state = get_device_state();
                state.last_sensor_update = Instant::now();
                
                // Mark model as needing update
                state.flags.model = true;
                
                // Add to history
                state.sensor_history.push_back((Instant::now(), state.sensors.clone()));
                if state.sensor_history.len() > 300 {
                    state.sensor_history.pop_front();
                }
            }
        }
    });
}
```

---

## 🧠 **Model System Thread**

```rust
// src/systems/model_system.rs

use std::sync::RwLock;
use std::time::Duration;
use crate::world_state::{WorldState, ModelType};
use crate::core::models::TemperatureField;

pub fn spawn_model_system(
    world: WorldState,
    neural_field: Arc<Mutex<TemperatureField>>,
) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            
            let mut state = world.write().unwrap();
            
            if state.flags.model {
                // Get current sensors
                let sensors = state.sensors.clone();
                let device_state = state.device_state.clone();
                
                drop(state); // Release lock for ML computation
                
                // Run neural field prediction (expensive!)
                let prediction = if let Ok(mut nf) = neural_field.try_lock() {
                    if nf.get_calibration_count() >= 5 {
                        nf.estimate_ambient(&sensors, &device_state)
                    } else {
                        // Fallback to linear
                        20.0 // TODO: linear model prediction
                    }
                } else {
                    20.0
                };
                
                // Update state with prediction
                let mut state = world.write().unwrap();
                state.ambient_temp = prediction;
                state.confidence = 0.85; // TODO: real confidence
                state.model_type = if state.neural_calibrations >= 5 {
                    ModelType::Neural
                } else {
                    ModelType::Linear
                };
                
                state.flags.model = false;
                state.flags.ui = true; // Mark UI for update
            }
        }
    });
}
```

---

## 🌐 **3D Sphere System**

```rust
// src/systems/sphere_system.rs

use std::sync::RwLock;
use std::time::Duration;
use crate::world_state::{WorldState, SpherePoint};
use crate::core::models::TemperatureField;

pub fn spawn_sphere_system(
    world: WorldState,
    neural_field: Arc<Mutex<TemperatureField>>,
) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(500)); // 2 FPS
            
            let state = world.read().unwrap();
            let sensors = state.sensors.clone();
            let device_state = state.device_state.clone();
            drop(state);
            
            // Sample sphere points
            let samples = sample_sphere(64); // 64 points
            
            let mut sphere_points = Vec::new();
            
            if let Ok(nf) = neural_field.try_lock() {
                for (x, y, z) in samples {
                    let temp = nf.estimate_temperature(
                        &sensors,
                        x, y, z,
                        &device_state
                    );
                    
                    sphere_points.push(SpherePoint { x, y, z, temperature: temp });
                }
            }
            
            // Update world state
            let mut state = world.write().unwrap();
            state.sphere_samples = sphere_points;
            state.flags.sphere = true;
        }
    });
}

fn sample_sphere(n: usize) -> Vec<(f32, f32, f32)> {
    let mut points = Vec::with_capacity(n);
    let golden_ratio = (1.0 + 5.0_f32.sqrt()) / 2.0;
    
    for i in 0..n {
        let theta = 2.0 * std::f32::consts::PI * (i as f32) / golden_ratio;
        let phi = ((i as f32 / n as f32) * 2.0 - 1.0).acos();
        
        let x = phi.sin() * theta.cos();
        let y = phi.sin() * theta.sin();
        let z = phi.cos();
        
        points.push((x, y, z));
    }
    
    points
}
```

---

## 📱 **QML Main Interface**

```qml
// qml/main.qml

import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Window 2.15

Window {
    visible: true
    width: 450
    height: 800
    title: "Ambient Learner"
    
    // Main layout
    SwipeView {
        id: swipeView
        anchors.fill: parent
        currentIndex: 0
        
        // Page 1: Live Temperature
        LiveView {
            temperature: qtobj.ambient_temp
            confidence: qtobj.confidence
            modelType: qtobj.model_type
            calibrationCount: qtobj.calibration_count
        }
        
        // Page 2: 3D Heat Sphere
        SphereView {
            sphereData: qtobj.sphere_data
        }
        
        // Page 3: History
        HistoryView {
        }
        
        // Page 4: Settings
        SettingsView {
        }
    }
    
    // Page indicator
    PageIndicator {
        count: swipeView.count
        currentIndex: swipeView.currentIndex
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
    }
    
    // Update timer (polls Rust backend)
    Timer {
        interval: 100
        running: true
        repeat: true
        onTriggered: qtobj.poll_updates()
    }
}
```

---

## 🎨 **Live View Page**

```qml
// qml/pages/LiveView.qml

import QtQuick 2.15
import QtQuick.Controls 2.15

Page {
    property real temperature: 20.0
    property real confidence: 0.0
    property string modelType: "Linear"
    property int calibrationCount: 0
    
    Rectangle {
        anchors.fill: parent
        color: "#1a1a1a"
        
        Column {
            anchors.centerIn: parent
            spacing: 20
            
            // Temperature display
            Text {
                text: temperature.toFixed(1) + "°C"
                font.pixelSize: 72
                font.bold: true
                color: temperatureColor(temperature)
                anchors.horizontalCenter: parent.horizontalCenter
            }
            
            // Confidence bar
            Rectangle {
                width: 300
                height: 8
                color: "#333"
                radius: 4
                
                Rectangle {
                    width: parent.width * confidence
                    height: parent.height
                    color: "#4CAF50"
                    radius: 4
                }
            }
            
            // Model info
            Text {
                text: "Model: " + modelType + " (" + calibrationCount + " calibrations)"
                font.pixelSize: 14
                color: "#888"
                anchors.horizontalCenter: parent.horizontalCenter
            }
            
            // Calibrate button
            Button {
                text: "🎯 Calibrate"
                anchors.horizontalCenter: parent.horizontalCenter
                onClicked: {
                    calibrationDialog.open()
                }
            }
        }
    }
    
    // Calibration dialog
    Dialog {
        id: calibrationDialog
        title: "Calibrate Model"
        
        TextField {
            id: tempInput
            placeholderText: "Enter temperature (°C)"
            inputMethodHints: Qt.ImhFormattedNumbersOnly
        }
        
        standardButtons: Dialog.Ok | Dialog.Cancel
        
        onAccepted: {
            qtobj.calibrate(parseFloat(tempInput.text))
        }
    }
    
    function temperatureColor(temp) {
        if (temp < 15) return "#2196F3"  // Blue
        if (temp < 20) return "#4CAF50"  // Green
        if (temp < 25) return "#FFC107"  // Yellow
        return "#F44336"  // Red
    }
}
```

---

## 🌐 **3D Sphere View**

```qml
// qml/pages/SphereView.qml

import QtQuick 2.15
import QtQuick3D 1.15

Page {
    property string sphereData: "[]"
    
    View3D {
        anchors.fill: parent
        
        PerspectiveCamera {
            position: Qt.vector3d(0, 0, 5)
            eulerRotation.x: -20
        }
        
        DirectionalLight {
            eulerRotation.x: -30
        }
        
        Model {
            id: heatSphere
            source: "#Sphere"
            
            materials: CustomMaterial {
                fragmentShader: "qrc:/shaders/heat_field.frag"
                
                // Pass sphere data to shader
                property var spherePoints: JSON.parse(sphereData)
            }
            
            // Rotation animation
            SequentialAnimation on eulerRotation.y {
                running: true
                loops: Animation.Infinite
                NumberAnimation {
                    from: 0
                    to: 360
                    duration: 10000
                }
            }
        }
    }
    
    // Instructions overlay
    Rectangle {
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        width: parent.width * 0.8
        height: 60
        color: "#80000000"
        radius: 10
        
        Text {
            anchors.centerIn: parent
            text: "Drag to rotate • Pinch to zoom"
            color: "white"
            font.pixelSize: 14
        }
    }
}
```

---

## 🎯 **main.rs Entry Point**

```rust
// src/main.rs

use qmetaobject::*;
use std::sync::{Arc, Mutex, RwLock};

mod qt_backend;
mod world_state;
mod systems;
mod core;  // Shared with GTK!
mod data;

use qt_backend::QtBackend;
use world_state::SystemState;

fn main() {
    // Create world state
    let world = Arc::new(RwLock::new(SystemState::new()));
    
    // Create models (shared with GTK version!)
    let neural_field = Arc::new(Mutex::new(
        core::models::TemperatureField::new(53)
    ));
    
    // Spawn systems
    systems::sensor_system::spawn_sensor_system(world.clone());
    systems::model_system::spawn_model_system(world.clone(), neural_field.clone());
    systems::sphere_system::spawn_sphere_system(world.clone(), neural_field.clone());
    systems::database_system::spawn_database_system(world.clone());
    
    // Create Qt backend
    let mut backend = QtBackend::default();
    backend.attach_state(world);
    
    let qtobj = QObjectBox::new(backend);
    
    // Setup QML engine
    qml_register_type::<QtBackend>(
        cstr::cstr!("AmbientLearner"), 1, 0, cstr::cstr!("Backend")
    );
    
    let mut engine = QmlEngine::new();
    engine.set_object_property("qtobj".into(), qtobj.pinned());
    engine.load_file("qml/main.qml".into());
    
    println!("✓ Qt/QML interface loaded");
    
    // Run
    engine.exec();
}
```

---

## 🚀 **Migration Strategy**

### Phase 1: Proof of Concept (1 week)
1. Create minimal Qt project
2. Wire WorldState + Sensor system
3. Display live temperature in QML
4. Verify threading works

### Phase 2: Core Features (2 weeks)
1. Add calibration
2. Wire neural field predictions
3. Implement all 5 pages
4. Database integration

### Phase 3: 3D Sphere (1 week)
1. Sample sphere points
2. Create fragment shader
3. Heat color mapping
4. Interactive rotation

### Phase 4: Polish (1 week)
1. Animations
2. Theme system
3. Settings
4. Error handling

---

## 💡 **Key Advantages**

**1. Code Reuse**
```
GTK Version:
src/core/     ← SHARED
src/ui/gtk4/  ← GTK-specific

Qt Version:
src/core/     ← SAME CODE!
src/qt_backend.rs ← Qt-specific
qml/          ← UI in QML
```

**2. Clean Threading**
- No GTK threading issues
- RwLock handles concurrency
- Dirty flags prevent over-updates

**3. 3D Rendering**
- Qt 3D built-in
- Hardware accelerated
- Shader support

**4. Mobile Ready**
- QML works on Android
- Touch gestures built-in
- Responsive by default

---

## 📊 **Performance Notes**

**RwLock vs Mutex:**
```rust
// UI reads (frequent, lightweight)
let state = world.read().unwrap();
let temp = state.ambient_temp;

// Sensor writes (infrequent, quick)
let mut state = world.write().unwrap();
state.sensors = new_readings;
```

**Dirty Flags:**
- Prevent unnecessary updates
- UI only redraws when `flags.ui = true`
- Sphere only recalculates when `flags.sphere = true`

---

## 🎉 **Summary**

**Your Qt architecture becomes:**
```
ThermalPoller → WorldState → QtBackend → QML
                    ↕
              NeuralField
                    ↕
               Database
```

**Benefits:**
- ✅ **Cross-platform** (Linux, Windows, macOS, Android)
- ✅ **Reuse GTK core** (80% code shared)
- ✅ **Clean threading** (no GUI thread violations)
- ✅ **3D visualization** (Qt 3D built-in)
- ✅ **Maintainable** (WorldState is single source of truth)

**Ready to start porting?** 🚀
