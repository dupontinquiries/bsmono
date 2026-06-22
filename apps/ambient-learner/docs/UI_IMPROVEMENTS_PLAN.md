# UI Improvements & Temperature Sphere Implementation Plan

## 🎯 **Phase 1: Move Neural Field to Main Display**

### Current State
```
Main display: Legacy model (position-based)
Neural field: Separate experimental section below
```

### Target State
```
Main display: Neural field (physics-aware) as PRIMARY
Legacy model: Keep running for comparison but demoted
```

### Implementation Steps

**1. Update temperature calculation priority**
```rust
// In UI update loop:
let temp = if neural_field_ready && cal_count >= 5 {
    // Use neural field (trained)
    neural_field.estimate_ambient(&sensors, &device_state)
} else {
    // Fallback to legacy
    model.predict(&sensors)
};
```

**2. Add model indicator**
```
🌡️ 14.2°C
Using: Neural Field (12 calibrations)  ← Show which model
```

**3. Keep legacy comparison in debug mode**
```
Main: 14.2°C (Neural)
Compare: 14.5°C (Legacy) - Δ0.3°C
```

---

## 🔮 **Phase 2: Wire Neural Field to Forecast**

### Current Forecast Flow
```
User calibrates → Legacy model updates
Forecast.add_observation(legacy_temp)
Forecast uses: 14-day history of LEGACY predictions
```

### Updated Forecast Flow
```
User calibrates → Both models update
Forecast.add_observation(neural_temp)  ← Use neural!
Forecast uses: 14-day history of NEURAL predictions
```

### Changes Needed

**1. Update forecast observation source**
```rust
// In calibration completion:
if let Ok(mut fm) = forecast_model.try_lock() {
    let neural_temp = neural_field.lock().unwrap()
        .estimate_ambient(&sensors, &device_state);
    
    fm.add_observation(forecast::TimeSeriesPoint {
        timestamp: chrono::Utc::now().timestamp(),
        temperature: neural_temp,  // Was: legacy_temp
        location_id: "current".to_string(),
    });
}
```

**2. Verify forecast algorithm**

**Already implemented:** ✅
- Linear regression for trend
- Hourly pattern (24h cycle)
- Daily pattern (7-day week)
- Confidence intervals

**Good!** Uses trendline statistics, not just averages.

---

## 🌐 **Phase 3: Temperature Sphere Visualization**

### Concept

**3D gradient sphere showing temperature distribution:**
```
        N (top)
       /   \
      /  🔴  \    ← Hot (display)
     |   📱   |   ← Phone center
      \  🔵  /    ← Cool (ambient)
       \   /
        S (bottom)
```

**Features:**
- Color gradient: Blue (cold) → Red (hot)
- 53 sensor positions mapped to sphere surface
- Rotate/drag to view from different angles
- Show NSEW cardinal points
- Ambient prediction at sphere edge

### SVG Implementation (Phase 3a - Simple)

**Pros:**
- Fast to implement
- Lightweight
- Good enough for testing
- Works in GTK

**Cons:**
- 2D projection only
- No interaction
- Limited visual appeal

**Code:**
```rust
fn generate_temperature_sphere_svg(
    sensors: &[f32],
    neural_field: &TemperatureField,
) -> String {
    // Project 53 sensors onto 2D circle
    // Color by temperature
    // Show gradient
    
    let svg = format!(r#"
        <svg viewBox="0 0 400 400">
            <!-- Gradient background -->
            <defs>
                <radialGradient id="tempGrad">
                    <stop offset="0%" stop-color="{center_color}"/>
                    <stop offset="100%" stop-color="{edge_color}"/>
                </radialGradient>
            </defs>
            
            <!-- Sphere -->
            <circle cx="200" cy="200" r="150" 
                    fill="url(#tempGrad)" opacity="0.3"/>
            
            <!-- Sensor points -->
            {sensor_circles}
            
            <!-- NSEW labels -->
            <text x="200" y="30">N</text>
            <text x="200" y="380">S</text>
            <text x="30" y="200">W</text>
            <text x="370" y="200">E</text>
        </svg>
    "#);
}
```

---

### WebGL Implementation (Phase 3b - Fancy)

**Future enhancement using WebGL in artifact:**

**Pros:**
- Real 3D
- Interactive rotation
- Smooth gradients
- Beautiful!

**Cons:**
- Complex implementation
- May slow UI (render in artifact)
- Requires Three.js or similar

**Code structure:**
```javascript
// In React artifact:
import * as THREE from 'three';

function TemperatureSphere({ sensors, predictions }) {
    // Create sphere geometry
    const geometry = new THREE.SphereGeometry(5, 32, 32);
    
    // Create vertex colors from temperatures
    const colors = new Float32Array(geometry.attributes.position.count * 3);
    for (let i = 0; i < sensors.length; i++) {
        const temp = sensors[i];
        const color = temperatureToColor(temp);
        colors[i * 3] = color.r;
        colors[i * 3 + 1] = color.g;
        colors[i * 3 + 2] = color.b;
    }
    
    // Smooth gradient using vertex interpolation
    geometry.setAttribute('color', 
        new THREE.BufferAttribute(colors, 3));
    
    // Render with rotation controls
}
```

---

## 📊 **Implementation Priority**

### Week 1: Core Functionality
- [x] Fix normalization bug
- [ ] Move neural field to main display
- [ ] Wire forecast to neural field
- [ ] Test accuracy improvements

### Week 2: Temperature Sphere (SVG)
- [ ] Design sphere layout
- [ ] Map 53 sensors to sphere surface
- [ ] Implement temperature → color gradient
- [ ] Add to Page 2 (Model page)
- [ ] Performance testing

### Week 3: Polish & Optimization
- [ ] Optimize SVG rendering
- [ ] Add caching
- [ ] Smooth color interpolation
- [ ] NSEW labels
- [ ] Legend/scale

### Week 4: Advanced (Optional)
- [ ] WebGL artifact version
- [ ] Interactive rotation
- [ ] Animated temperature changes
- [ ] Export as image

---

## 🎨 **Temperature Sphere Design**

### Color Mapping

**Temperature to RGB:**
```rust
fn temperature_to_color(temp: f32) -> (u8, u8, u8) {
    // Blue → Cyan → Green → Yellow → Red
    // 0°C → 10°C → 20°C → 30°C → 40°C
    
    let normalized = (temp - 10.0) / 30.0;  // 0-40°C → 0-1
    let clamped = normalized.clamp(0.0, 1.0);
    
    if clamped < 0.25 {
        // Blue → Cyan
        let t = clamped * 4.0;
        (0, (t * 255.0) as u8, 255)
    } else if clamped < 0.5 {
        // Cyan → Green
        let t = (clamped - 0.25) * 4.0;
        (0, 255, ((1.0 - t) * 255.0) as u8)
    } else if clamped < 0.75 {
        // Green → Yellow
        let t = (clamped - 0.5) * 4.0;
        ((t * 255.0) as u8, 255, 0)
    } else {
        // Yellow → Red
        let t = (clamped - 0.75) * 4.0;
        (255, ((1.0 - t) * 255.0) as u8, 0)
    }
}
```

### Sensor Position Mapping

**Problem:** We have 53 sensors, need to map to sphere

**Approach 1: Even distribution**
```rust
// Fibonacci sphere (even distribution)
fn map_sensor_to_sphere_position(index: usize, total: usize) -> (f32, f32, f32) {
    let golden_ratio = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let theta = 2.0 * PI * (index as f32) / golden_ratio;
    let phi = ((index as f32 / total as f32) * 2.0 - 1.0).acos();
    
    let x = phi.sin() * theta.cos();
    let y = phi.sin() * theta.sin();
    let z = phi.cos();
    
    (x, y, z)
}
```

**Approach 2: Physical mapping**
```rust
// Map based on actual sensor names
// thermal_zone0 → CPU → North
// thermal_zone1 → GPU → South
// battery → Bottom
// etc.

match sensor_name {
    "cpu" => (0.0, 0.8, 0.6),     // Top front
    "gpu" => (0.0, -0.8, 0.6),    // Bottom front
    "battery" => (0.0, 0.0, -1.0), // Back
    _ => fibonacci_position(i)     // Fallback
}
```

---

## 🚀 **Quick Win: SVG Sphere**

### Minimal Implementation

```rust
pub fn generate_temperature_sphere(
    sensors: &[f32],
    width: u32,
    height: u32,
) -> String {
    let center_x = width / 2;
    let center_y = height / 2;
    let radius = (width.min(height) / 2) as f32 * 0.8;
    
    let mut svg = format!(
        r#"<svg width="{}" height="{}" viewBox="0 0 {} {}">"#,
        width, height, width, height
    );
    
    // Background gradient (average temp)
    let avg_temp: f32 = sensors.iter().sum::<f32>() / sensors.len() as f32;
    let bg_color = temperature_to_hex(avg_temp);
    
    svg.push_str(&format!(
        r#"<circle cx="{}" cy="{}" r="{}" fill="{}" opacity="0.2"/>"#,
        center_x, center_y, radius, bg_color
    ));
    
    // Sensor points
    for (i, &temp) in sensors.iter().enumerate() {
        let (x, y, z) = map_sensor_position(i, sensors.len());
        
        // Project 3D → 2D (simple orthographic)
        let px = center_x as f32 + x * radius;
        let py = center_y as f32 + y * radius;
        
        // Size based on z-depth (larger = closer)
        let size = 3.0 + z * 2.0;
        
        let color = temperature_to_hex(temp);
        
        svg.push_str(&format!(
            r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{}"/>"#,
            px, py, size, color
        ));
    }
    
    // NSEW labels
    svg.push_str(&format!(
        r#"<text x="{}" y="{}" text-anchor="middle">N</text>"#,
        center_x, 20
    ));
    // ... S, E, W
    
    svg.push_str("</svg>");
    svg
}
```

---

## 📈 **Performance Considerations**

### SVG Sphere
- **Rendering:** ~1-2ms for 53 circles
- **Update rate:** Every 100ms (UI refresh)
- **Impact:** Minimal (<1% CPU)

**Optimization:**
- Only regenerate when sensors change significantly
- Cache SVG string
- Use CSS transforms for rotation (if needed)

### WebGL Sphere (future)
- **Rendering:** ~5-10ms initial
- **Update rate:** 60 FPS possible
- **Impact:** Moderate (5-10% CPU)

**Optimization:**
- Render in separate thread
- Update texture instead of geometry
- LOD (level of detail) based on zoom

---

## 🎯 **Next Steps**

### Immediate (This Session)
1. Move neural field to main display
2. Update forecast to use neural predictions
3. Create basic SVG sphere prototype

### Short Term (Next Session)
1. Refine sphere visual design
2. Add proper sensor positioning
3. Test performance
4. Add to Page 2

### Long Term
1. WebGL version in artifact
2. Interactive rotation
3. Animation
4. Export/share

---

## 💡 **Design Questions**

**Q: Where to put temperature sphere?**
**A:** Page 2 (Model page) - replaces or supplements network diagram

**Q: Real-time or cached?**
**A:** Cached - update every 1s, only if sensors changed

**Q: Show all 53 sensors or aggregate?**
**A:** Start with all 53, add toggle for "simplified view" later

**Q: Color scheme?**
**A:** Blue→Red (cold→hot), with configurable scale

Let me know which phase you want to start with!
