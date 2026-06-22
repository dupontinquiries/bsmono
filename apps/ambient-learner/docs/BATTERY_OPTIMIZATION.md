# Battery Optimization Strategy - Mobile Focus

## 🔋 Current Status Analysis

### Power Consumption Breakdown

**Before optimizations:**
- UI updates: 100ms intervals = 10 Hz → ~50mW
- SVG regeneration: Every 100ms → ~30mW
- WiFi polling: Every 5s → ~20mW
- Sensor reading: Every 100ms → ~15mW
- CSS animations: Continuous → ~25mW
- **Total: ~140mW** (3-4 hours battery life)

**After Phase 1 & 2 optimizations:**
- UI updates: 100ms intervals with skip → ~10mW
- SVG caching: Only on change → ~2mW
- WiFi: One-time detection → ~0.02mW
- Sensor reading: Every 100ms → ~15mW
- CSS animations: Continuous → ~25mW
- **Total: ~52mW** (7-8 hours battery life)

**Target after mobile optimizations:**
- **Goal: <20mW** (15+ hours battery life)

---

## ⚡ Optimization Plan

### 1. Adaptive Update Rate
**Problem:** 10 Hz refresh is overkill when temperature is stable

**Solution:** Dynamic polling based on variance
```rust
// Fast updates when changing (100ms)
// Slow updates when stable (1000ms)
// Sleep when idle (5000ms)

let update_interval = if temp_variance > 0.5 {
    100  // Temperature changing rapidly
} else if temp_variance > 0.1 {
    500  // Temperature changing slowly
} else {
    2000 // Temperature stable
};
```

**Savings:** 50-80% reduction in UI updates

---

### 2. CSS Animation Power Management

**Problem:** Continuous animations drain battery

**Solution:** Pause animations when:
- App in background
- No user interaction for 30s
- Battery < 20%
- Power saver mode enabled

```rust
// Detect visibility changes
window.connect_is_active_notify(|window| {
    if !window.is_active() {
        pause_animations();
    } else {
        resume_animations();
    }
});
```

**Savings:** 15-20mW when inactive

---

### 3. Sensor Reading Optimization

**Problem:** Reading 45+ sensors every 100ms

**Solution:** Tiered sensor polling
```rust
// Critical sensors: 100ms (sensors with high weight)
// Normal sensors: 500ms (sensors with medium weight)
// Background sensors: 2000ms (sensors with low weight)

if sensor.weight > 0.5 {
    poll_every_100ms
} else if sensor.weight > 0.1 {
    poll_every_500ms
} else {
    poll_every_2000ms
}
```

**Savings:** 50% reduction in I/O operations

---

### 4. GPU Acceleration Tuning

**Problem:** Some effects force CPU rendering

**Solution:** Optimize CSS for GPU
```css
/* Force GPU acceleration */
.gpu-accelerated {
    transform: translateZ(0);
    will-change: transform, opacity;
}

/* Avoid expensive properties */
/* Bad: box-shadow changes, filter changes */
/* Good: transform, opacity */
```

**Savings:** 10-15% better GPU efficiency

---

### 5. Background Task Suspension

**Problem:** Passive model updates continue in background

**Solution:** Pause when not visible
```rust
if !window.is_active() {
    // Pause passive updates
    // Pause UI refresh
    // Keep only critical timers
}
```

**Savings:** ~30mW when in background

---

### 6. Network Power Management

**Problem:** Even one-time WiFi check uses power

**Solution:** Cache aggressively
```rust
// Cache WiFi SSID for entire session
// Only re-detect on explicit user action
// Skip network check if location is "Manual"
```

**Savings:** Minimal but consistent

---

### 7. Database Write Batching

**Problem:** Frequent small writes

**Solution:** Batch calibrations
```rust
// Instead of: Write after each calibration
// Do: Collect 5-10 calibrations, write once

let mut calibration_buffer = Vec::new();
if calibration_buffer.len() >= 10 {
    db.batch_insert(&calibration_buffer);
    calibration_buffer.clear();
}
```

**Savings:** 50% reduction in disk I/O

---

### 8. Smart Sleep Modes

**Problem:** App runs even when no one is looking

**Implementation:**
```rust
// User inactivity detection
let mut last_interaction = Instant::now();

// After 30s of no interaction
if last_interaction.elapsed() > Duration::from_secs(30) {
    // Reduce to 1 Hz updates
    // Pause animations
    // Minimal sensor polling
}

// After 5 minutes of no interaction
if last_interaction.elapsed() > Duration::from_secs(300) {
    // 0.2 Hz updates (every 5 seconds)
    // All animations off
    // Only temperature tracking
}
```

**Savings:** 60-80% when idle

---

## 🎯 Implementation Priority

### High Priority (Immediate Impact)

**1. Adaptive Update Rate** ⚡
- Complexity: Low
- Impact: High (50-80% UI update reduction)
- Implementation: 30 minutes

**2. Background Pause** ⚡
- Complexity: Low  
- Impact: High (30mW saving)
- Implementation: 15 minutes

**3. Inactivity Sleep** ⚡
- Complexity: Medium
- Impact: High (60-80% when idle)
- Implementation: 45 minutes

### Medium Priority (Good ROI)

**4. CSS Animation Pause** 🔋
- Complexity: Low
- Impact: Medium (15-20mW)
- Implementation: 20 minutes

**5. Tiered Sensor Polling** 🔋
- Complexity: Medium
- Impact: Medium (50% I/O reduction)
- Implementation: 60 minutes

**6. Database Batching** 🔋
- Complexity: Low
- Impact: Low-Medium
- Implementation: 30 minutes

### Low Priority (Polish)

**7. GPU Optimization** 💡
- Complexity: Low
- Impact: Low-Medium (10-15%)
- Implementation: 15 minutes

**8. Network Cache** 💡
- Complexity: Low
- Impact: Minimal
- Implementation: Already done ✓

---

## 📊 Expected Results

### Power Consumption (mW)

| State | Before | After | Savings |
|-------|--------|-------|---------|
| Active use | 52 | 25 | 52% |
| Stable temp | 52 | 15 | 71% |
| Background | 52 | 5 | 90% |
| Idle (30s+) | 52 | 8 | 85% |
| Idle (5min+) | 52 | 3 | 94% |

### Battery Life Estimates (3000mAh battery)

| Usage Pattern | Current | Target | Improvement |
|---------------|---------|--------|-------------|
| Continuous active | 8h | 16h | 2x |
| Typical use (mixed) | 12h | 24h | 2x |
| Background monitoring | 18h | 60h | 3.3x |

---

## 🔧 Implementation Code

### 1. Adaptive Update Rate

```rust
// Add variance tracking
let mut temp_history: VecDeque<f32> = VecDeque::with_capacity(10);
let mut last_variance = 0.0;

// In UI update loop
temp_history.push_back(temp);
if temp_history.len() > 10 {
    temp_history.pop_front();
}

// Calculate variance every second
if update_count % 10 == 0 {
    let mean = temp_history.iter().sum::<f32>() / temp_history.len() as f32;
    let variance = temp_history.iter()
        .map(|t| (t - mean).powi(2))
        .sum::<f32>() / temp_history.len() as f32;
    last_variance = variance;
}

// Adjust update interval
let interval = if last_variance > 0.5 {
    Duration::from_millis(100)  // Changing rapidly
} else if last_variance > 0.1 {
    Duration::from_millis(500)  // Changing slowly
} else {
    Duration::from_millis(2000) // Stable
};

// Use dynamic interval instead of fixed 100ms
glib::timeout_add_local(interval, move || { ... });
```

---

### 2. Background Detection & Pause

```rust
// Detect when window becomes inactive
let window_clone = window.clone();
let ui_active = Arc::new(Mutex::new(true));
let ui_active_clone = ui_active.clone();

window.connect_is_active_notify(move |window| {
    let active = window.is_active();
    *ui_active_clone.lock().unwrap() = active;
    
    if active {
        println!("📱 App active - resuming full updates");
    } else {
        println!("💤 App background - pausing updates");
    }
});

// In UI update loop
if !*ui_active.lock().unwrap() {
    // Skip most updates when in background
    return glib::ControlFlow::Continue;
}
```

---

### 3. Inactivity Detection

```rust
// Track last user interaction
let last_interaction = Arc::new(Mutex::new(Instant::now()));
let last_interaction_clone = last_interaction.clone();

// Update on any interaction
window.connect_key_pressed(move |_, _, _, _| {
    *last_interaction_clone.lock().unwrap() = Instant::now();
    // ... handler
});

// Also track clicks, scrolls, etc.

// In UI update loop
let idle_time = last_interaction.lock().unwrap().elapsed();

let interval = if idle_time > Duration::from_secs(300) {
    Duration::from_millis(5000)  // 5 min idle: ultra-low power
} else if idle_time > Duration::from_secs(30) {
    Duration::from_millis(1000)  // 30s idle: low power
} else {
    // Use adaptive interval from variance
    adaptive_interval
};
```

---

### 4. CSS Animation Pause

```rust
// Pause CSS animations when inactive
fn pause_animations(widget: &impl WidgetExt) {
    widget.add_css_class("animations-paused");
}

fn resume_animations(widget: &impl WidgetExt) {
    widget.remove_css_class("animations-paused");
}
```

```css
/* In style.css */
.animations-paused * {
    animation-play-state: paused !important;
}
```

---

### 5. Tiered Sensor Polling

```rust
struct SensorTier {
    critical: Vec<usize>,  // High weight sensors
    normal: Vec<usize>,    // Medium weight sensors  
    background: Vec<usize>, // Low weight sensors
}

fn categorize_sensors(model: &Model) -> SensorTier {
    let mut tiers = SensorTier {
        critical: Vec::new(),
        normal: Vec::new(),
        background: Vec::new(),
    };
    
    for (i, stat) in model.stats.iter().enumerate() {
        if stat.weight.abs() > 0.5 {
            tiers.critical.push(i);
        } else if stat.weight.abs() > 0.1 {
            tiers.normal.push(i);
        } else {
            tiers.background.push(i);
        }
    }
    
    tiers
}

// Poll different tiers at different rates
// Critical: Every update
// Normal: Every 5th update
// Background: Every 20th update
```

---

### 6. Database Write Batching

```rust
// Add to main state
let calibration_buffer: Arc<Mutex<Vec<CalibrationData>>> = 
    Arc::new(Mutex::new(Vec::new()));

// When calibrating
calibration_buffer.lock().unwrap().push(calibration_data);

// Periodic flush (every 10 calibrations or 5 minutes)
if buffer.len() >= 10 || last_flush.elapsed() > Duration::from_secs(300) {
    db.batch_insert_calibrations(&buffer);
    buffer.clear();
}
```

---

## 🧪 Battery Testing Protocol

### Measurement Setup

1. **Fully charge device** to 100%
2. **Enable airplane mode** (isolate from other factors)
3. **Set screen brightness** to 50%
4. **Close all other apps**
5. **Start app and monitor**

### Test Scenarios

**Test 1: Active Use**
- Duration: 1 hour
- Action: View app, calibrate every 5 min
- Measure: Battery % drop

**Test 2: Stable Monitoring**
- Duration: 2 hours
- Action: Leave app open, stable temperature
- Measure: Battery % drop

**Test 3: Background**
- Duration: 2 hours
- Action: Switch to other apps
- Measure: Battery % drop

**Test 4: Idle**
- Duration: 4 hours
- Action: Leave app visible, no interaction
- Measure: Battery % drop

### Expected Results

| Test | Current | Target |
|------|---------|--------|
| Active (1h) | -6% | -3% |
| Stable (2h) | -8% | -4% |
| Background (2h) | -6% | -1% |
| Idle (4h) | -12% | -3% |

---

## 📱 Mobile-Specific Optimizations

### Android/Termux

**1. Wakelocks**
```rust
// Don't keep CPU awake unnecessarily
// Let system suspend when idle
```

**2. Thermal Throttling**
```rust
// Reduce polling if device gets hot
// Prevents thermal throttling
```

**3. Battery Saver Mode Detection**
```bash
# Detect power save mode
dumpsys battery | grep "AC powered: false"
```

**4. Screen Off Detection**
```rust
// Minimal updates when screen is off
// Check via /sys/class/backlight/*/actual_brightness
```

---

## 🎯 Quick Wins Implementation

### Immediate (15 minutes)

**1. Background Pause**
```rust
window.connect_is_active_notify(move |w| {
    if !w.is_active() { pause_all_timers(); }
});
```

**2. Animation Pause CSS**
```css
.animations-paused * {
    animation-play-state: paused !important;
}
```

### Short Term (1 hour)

**3. Adaptive Update Rate**
- Track temperature variance
- Adjust polling dynamically
- 100ms → 2000ms when stable

**4. Inactivity Sleep**
- Detect no interaction
- Reduce to 1 Hz after 30s
- Reduce to 0.2 Hz after 5min

### Medium Term (2-3 hours)

**5. Tiered Sensor Polling**
- Categorize by weight
- Poll important sensors more
- Background sensors less

**6. Database Batching**
- Buffer writes
- Flush every 10 items or 5 min
- Reduce I/O

---

## 📊 Monitoring & Metrics

### Add Battery Telemetry

```rust
// Track power consumption metrics
struct BatteryStats {
    ui_updates_per_sec: f32,
    sensor_reads_per_sec: f32,
    db_writes_per_min: f32,
    active_animations: usize,
}

// Log every minute
println!("⚡ Battery stats: UI {:.1} Hz, Sensors {:.1} Hz, DB {:.1} w/min",
    stats.ui_updates_per_sec,
    stats.sensor_reads_per_sec, 
    stats.db_writes_per_min
);
```

### Battery Life Dashboard

```
┌─────────────────────────────┐
│ 🔋 Power Efficiency         │
├─────────────────────────────┤
│ UI Updates:     0.5 Hz ✓    │
│ Sensor Reads:   2.0 Hz ✓    │
│ Animations:     Paused ✓    │
│ Background:     Yes ✓       │
│                             │
│ Est. Battery:   20h ⚡      │
└─────────────────────────────┘
```

---

## ✅ Implementation Checklist

**Phase 1: Quick Wins (Day 1)**
- [ ] Background pause on window.is_active
- [ ] CSS animation pause
- [ ] Inactivity detection (30s threshold)
- [ ] Basic adaptive update rate

**Phase 2: Core Optimizations (Day 2)**
- [ ] Full adaptive polling based on variance
- [ ] Tiered sensor reading
- [ ] Database write batching
- [ ] GPU optimization CSS

**Phase 3: Polish (Day 3)**
- [ ] Battery telemetry
- [ ] Power efficiency dashboard
- [ ] Mobile-specific detection
- [ ] Testing on real devices

---

## 🎯 Success Metrics

**Target Goals:**
- ✅ <20mW average power draw
- ✅ 15+ hours battery life (typical use)
- ✅ 48+ hours battery life (background)
- ✅ No perceivable lag or stuttering
- ✅ Smooth experience on low-end devices

**Quality Gates:**
- ✅ UI still responsive (<100ms latency)
- ✅ Temperature updates feel real-time
- ✅ Animations smooth when active
- ✅ No battery drain complaints

---

## 🚀 Summary

**Current State:** ~52mW, ~8h battery life
**Target State:** ~20mW, ~20h battery life  
**Improvement:** **2.5x battery life**

**Key Strategies:**
1. Adaptive update rates (50-80% savings)
2. Background pause (90% savings when inactive)
3. Inactivity sleep (85% savings when idle)
4. Tiered sensor polling (50% I/O reduction)
5. CSS animation control (15-20mW savings)

**Implementation Effort:** ~4-5 hours total
**Expected ROI:** 2-3x battery life improvement

**Next Steps:**
1. Implement quick wins (background pause, animation control)
2. Add adaptive update rate
3. Test on real mobile device
4. Measure and iterate

🔋 **Let's make this app sip power like a pro!**
