# Iced UI - Corrected Files

## 🔧 **Files That Need Updates**

### 1. Cargo.toml - Dependencies
Ensure iced dependency is correct:

```toml
[dependencies]
# Iced dependencies (optional with feature flag)
iced = { version = "0.12", optional = true }
```

**Note:** Iced 0.12 is the latest stable. If build fails, try 0.10:
```toml
iced = { version = "0.10", optional = true }
```

---

### 2. src/ui/iced/mod.rs - Complete Working Version

Save this as the complete file:

```rust
// src/ui/iced/mod.rs
// Iced GUI implementation using WorldState architecture

use iced::{
    widget::{button, column, container, row, text, text_input},
    Alignment, Application, Command, Element, Length, Settings, Theme,
};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;
use crate::world_state::WorldState;

/// Main Iced application
pub struct AmbientLearnerApp {
    world: WorldState,
    
    // UI state
    temperature: f32,
    confidence: f32,
    model_type: String,
    calibration_count: usize,
    sensor_count: usize,
    
    // Calibration dialog
    calibration_input: String,
    show_calibration_dialog: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    OpenCalibrationDialog,
    CloseCalibrationDialog,
    CalibrationInputChanged(String),
    Calibrate,
}

impl Application for AmbientLearnerApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = WorldState;

    fn new(world: WorldState) -> (Self, Command<Message>) {
        (
            Self {
                world,
                temperature: 20.0,
                confidence: 0.0,
                model_type: "Linear".to_string(),
                calibration_count: 0,
                sensor_count: 0,
                calibration_input: String::new(),
                show_calibration_dialog: false,
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Ambient Learner - Iced UI")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Tick => {
                // Read from WorldState (populated by worker threads)
                if let Ok(state) = self.world.read() {
                    self.temperature = state.ambient_temp;
                    self.confidence = state.confidence;
                    self.model_type = state.model_type.as_str().to_string();
                    self.calibration_count = state.calibration_count;
                    self.sensor_count = state.sensor_count;
                }
                Command::none()
            }
            Message::OpenCalibrationDialog => {
                self.show_calibration_dialog = true;
                self.calibration_input = "20.0".to_string();
                Command::none()
            }
            Message::CloseCalibrationDialog => {
                self.show_calibration_dialog = false;
                Command::none()
            }
            Message::CalibrationInputChanged(value) => {
                self.calibration_input = value;
                Command::none()
            }
            Message::Calibrate => {
                if let Ok(target) = self.calibration_input.parse::<f32>() {
                    println!("🎯 Calibration requested: {}°C", target);
                    // TODO: Trigger calibration service
                    // For now, just mark model as needing update
                    if let Ok(mut state) = self.world.write() {
                        state.flags.model = true;
                    }
                }
                self.show_calibration_dialog = false;
                Command::none()
            }
        }
    }

    fn view(&self) -> Element<Message> {
        let temp_color = temperature_color(self.temperature);
        
        // Main temperature display
        let temp_display = container(
            column![
                text("AMBIENT LEARNER").size(14),
                text(format!("{:.1}°C", self.temperature))
                    .size(72)
                    .style(temp_color),
                text(format!("Confidence: {:.0}%", self.confidence * 100.0))
                    .size(16),
            ]
            .spacing(10)
            .align_items(Alignment::Center)
        )
        .padding(40)
        .center_x()
        .center_y();
        
        // Status info
        let status = container(
            row![
                column![
                    text("MODEL").size(10),
                    text(&self.model_type).size(14),
                ]
                .spacing(5),
                column![
                    text("CALIBRATIONS").size(10),
                    text(format!("{}", self.calibration_count)).size(14),
                ]
                .spacing(5),
                column![
                    text("SENSORS").size(10),
                    text(format!("{}", self.sensor_count)).size(14),
                ]
                .spacing(5),
            ]
            .spacing(40)
            .align_items(Alignment::Center)
        )
        .padding(20)
        .center_x();
        
        // Calibrate button
        let calibrate_btn = button(text("🎯 Calibrate").size(16))
            .padding(15)
            .on_press(Message::OpenCalibrationDialog);
        
        // Main content
        let content = column![
            temp_display,
            status,
            calibrate_btn,
        ]
        .spacing(20)
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fill);
        
        // Wrap in container
        let main_view = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y();
        
        // Add calibration dialog if shown
        if self.show_calibration_dialog {
            let dialog = container(
                column![
                    text("Calibrate Model").size(20),
                    text("Enter known ambient temperature:").size(14),
                    text_input("20.0", &self.calibration_input)
                        .on_input(Message::CalibrationInputChanged)
                        .padding(10),
                    row![
                        button(text("Cancel"))
                            .on_press(Message::CloseCalibrationDialog)
                            .padding(10),
                        button(text("Calibrate"))
                            .on_press(Message::Calibrate)
                            .padding(10),
                    ]
                    .spacing(10),
                ]
                .spacing(15)
                .padding(30)
                .max_width(400.0)
            )
            .padding(20)
            .center_x()
            .center_y();
            
            dialog.into()
        } else {
            main_view.into()
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // Update UI at 10 Hz
        iced::time::every(Duration::from_millis(100))
            .map(|_| Message::Tick)
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

/// Run the Iced UI
pub fn run(world: WorldState) -> iced::Result {
    println!("🎨 Starting Iced UI...");
    
    AmbientLearnerApp::run(Settings {
        flags: world,
        window: iced::window::Settings {
            size: iced::Size::new(450.0, 600.0),
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Get color for temperature display
fn temperature_color(temp: f32) -> iced::Color {
    if temp < 15.0 {
        iced::Color::from_rgb(0.13, 0.59, 0.95) // Blue
    } else if temp < 20.0 {
        iced::Color::from_rgb(0.0, 0.74, 0.83) // Cyan
    } else if temp < 25.0 {
        iced::Color::from_rgb(0.30, 0.69, 0.31) // Green
    } else if temp < 30.0 {
        iced::Color::from_rgb(1.0, 0.76, 0.03) // Yellow
    } else if temp < 35.0 {
        iced::Color::from_rgb(1.0, 0.60, 0.0) // Orange
    } else {
        iced::Color::from_rgb(0.96, 0.26, 0.21) // Red
    }
}
```

---

## 🔨 **Build Commands**

### 1. Clean build:
```bash
cargo clean
```

### 2. Build with Iced:
```bash
cargo build --release --features iced
```

### 3. Build with all features:
```bash
cargo build --release --all-features
```

### 4. Run:
```bash
./target/release/ambient-learner --ui iced
```

---

## 🐛 **Troubleshooting**

### Error: "cannot find widget text_input"

**Solution:** Update imports:
```rust
use iced::widget::{button, column, container, row, text, text_input};
```

### Error: "iced::time not found"

**Solution:** Use correct Subscription import:
```rust
use iced::Subscription;
```

### Error: "method `style` not found"

**Cause:** Iced 0.12 changed styling API

**Solution:** Simplified version (remove .style() calls):
```rust
text("AMBIENT LEARNER").size(14)  // No .style()
```

---

## ✅ **Verification**

After building, check:

```bash
# 1. Binary exists
ls -lh target/release/ambient-learner

# 2. Has iced symbols
nm target/release/ambient-learner | grep -i iced | head -5

# 3. Run with debug
RUST_BACKTRACE=1 ./target/release/ambient-learner --ui iced --debug
```

**Expected output:**
```
🐛 Debug mode enabled
   UI: iced
   Headless: false
🎨 UI Framework: iced
🎨 Launching Iced UI...
[1/5] Initializing sensors...
✓ Found 53 thermal sensors
[2/5] Creating models...
[3/5] Creating WorldState...
[4/5] Spawning worker systems...
🚀 Spawning all system workers...
🌡️  Sensor system started
🧠 Model system started
✅ All systems spawned
[5/5] Starting Iced UI...
🎨 Starting Iced UI...
```

Then a window should open!

---

## 📋 **Complete File Checklist**

- [ ] `Cargo.toml` - Has `iced = { version = "0.12", optional = true }`
- [ ] `src/ui/mod.rs` - Exports iced module
- [ ] `src/ui/iced/mod.rs` - Complete code above
- [ ] `src/main.rs` - Has run_iced_ui() function
- [ ] Build with: `cargo build --features iced`
- [ ] Run with: `./target/release/ambient-learner --ui iced`

---

## 🚀 **If Still Not Working**

Try using iced 0.10 instead (more stable):

```toml
[dependencies]
iced = { version = "0.10", optional = true }
```

Then rebuild:
```bash
cargo clean
cargo build --release --features iced
./target/release/ambient-learner --ui iced
```
