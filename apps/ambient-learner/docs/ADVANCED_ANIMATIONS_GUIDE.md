# Advanced Features: Snap Algorithms & Inertial Animations

## 🎯 **Your Advanced Requests**

### 1. Snap Scroll Optimization
**Question:** Dijkstra or MST for snap calculations?

**Answer:** Neither! For snap scrolling, we want:
- **Simple distance calculation** (faster than Dijkstra)
- **Linear scan** (O(n) where n = number of snap points, typically <10)
- **Spring physics** for smooth animation

**Why not Dijkstra/MST:**
- Dijkstra: Overkill for 1D distance (designed for graphs)
- MST: Irrelevant (we're not connecting points)
- Snap points are already sorted in 1D space

**Best algorithm:**
```rust
// O(n) linear search - fast enough for <100 snap points
fn find_nearest_snap(current: f32, snap_points: &[f32]) -> f32 {
    snap_points
        .iter()
        .min_by(|a, b| {
            (current - **a).abs()
                .partial_cmp(&(current - **b).abs())
                .unwrap()
        })
        .copied()
        .unwrap_or(current)
}
```

**Optimization for many snap points (>100):**
```rust
// Binary search: O(log n) - if snap points are sorted
fn find_nearest_snap_binary(current: f32, snap_points: &[f32]) -> f32 {
    let idx = snap_points.binary_search_by(|probe| {
        probe.partial_cmp(&current).unwrap()
    }).unwrap_or_else(|e| e);
    
    // Check neighbors
    let candidates = [
        idx.saturating_sub(1),
        idx,
        (idx + 1).min(snap_points.len() - 1),
    ];
    
    candidates
        .iter()
        .filter_map(|&i| snap_points.get(i))
        .min_by(|a, b| {
            (current - **a).abs()
                .partial_cmp(&(current - **b).abs())
                .unwrap()
        })
        .copied()
        .unwrap_or(current)
}
```

---

### 2. Inertial Jiggle Animation with Lerp

**Yes!** We can add physics-based animations with density simulation:

```rust
/// Inertial animation with configurable mass
pub struct InertialLerp {
    pub current: f32,
    pub target: f32,
    pub velocity: f32,
    pub mass: f32,          // Density simulation (1.0 = normal, 5.0 = heavy)
    pub spring_constant: f32,
    pub damping: f32,
    pub use_density: bool,  // Toggle for unified vs density-based
}

impl InertialLerp {
    pub fn new(initial: f32, mass: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            mass,
            spring_constant: 180.0,  // Stiffness
            damping: 12.0,           // Friction
            use_density: false,
        }
    }
    
    /// Update with spring physics
    pub fn update(&mut self, delta_time: f32) -> f32 {
        if !self.use_density {
            // Simple lerp (lightweight, no jiggle)
            self.current += (self.target - self.current) * 0.15;
        } else {
            // Physics-based spring (with jiggle)
            // F = -kx - bv (Hooke's law + damping)
            let displacement = self.current - self.target;
            let spring_force = -self.spring_constant * displacement;
            let damping_force = -self.damping * self.velocity;
            
            // F = ma → a = F/m
            let acceleration = (spring_force + damping_force) / self.mass;
            
            // Integrate velocity and position
            self.velocity += acceleration * delta_time;
            self.current += self.velocity * delta_time;
            
            // Stop if close enough and slow enough
            if displacement.abs() < 0.01 && self.velocity.abs() < 0.01 {
                self.current = self.target;
                self.velocity = 0.0;
            }
        }
        
        self.current
    }
    
    /// Set new target
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }
    
    /// Toggle density-based physics
    pub fn set_use_density(&mut self, enabled: bool) {
        self.use_density = enabled;
        if !enabled {
            self.velocity = 0.0;  // Reset velocity when switching to lerp
        }
    }
}
```

---

### 3. Example: Heavy vs Light Components

```rust
// Light component (tabs, buttons)
let tab_animator = InertialLerp::new(0.0, 1.0);  // mass = 1.0

// Heavy component (panels, cards)
let panel_animator = InertialLerp::new(0.0, 5.0);  // mass = 5.0

// Very heavy (full page transitions)
let page_animator = InertialLerp::new(0.0, 10.0);  // mass = 10.0
```

**Result:**
- Light: Quick, snappy response
- Heavy: Slower acceleration, more momentum, satisfying bounce
- Very heavy: Sluggish start, carries momentum, feels "weighty"

---

### 4. Complete Animation System

```rust
pub struct AnimationSystem {
    animations: HashMap<String, InertialLerp>,
    use_density_globally: bool,
}

impl AnimationSystem {
    pub fn new() -> Self {
        Self {
            animations: HashMap::new(),
            use_density_globally: false,
        }
    }
    
    /// Register an animated property
    pub fn register(&mut self, name: &str, initial: f32, mass: f32) {
        let mut lerp = InertialLerp::new(initial, mass);
        lerp.set_use_density(self.use_density_globally);
        self.animations.insert(name.to_string(), lerp);
    }
    
    /// Animate to target
    pub fn animate_to(&mut self, name: &str, target: f32) {
        if let Some(lerp) = self.animations.get_mut(name) {
            lerp.set_target(target);
        }
    }
    
    /// Update all animations
    pub fn update(&mut self, delta_time: f32) -> HashMap<String, f32> {
        let mut values = HashMap::new();
        
        for (name, lerp) in &mut self.animations {
            let value = lerp.update(delta_time);
            values.insert(name.clone(), value);
        }
        
        values
    }
    
    /// Toggle density-based physics globally
    pub fn toggle_density_mode(&mut self) {
        self.use_density_globally = !self.use_density_globally;
        
        for lerp in self.animations.values_mut() {
            lerp.set_use_density(self.use_density_globally);
        }
        
        println!("🎭 Animation mode: {}", 
            if self.use_density_globally { "Density-based (jiggly)" } 
            else { "Simple lerp (smooth)" }
        );
    }
}
```

---

### 5. Usage in GTK

```rust
// In main.rs, create animation system
let animation_system = Rc::new(RefCell::new(AnimationSystem::new()));

// Register animations with different masses
animation_system.borrow_mut().register("column_width", 60.0, 3.0);
animation_system.borrow_mut().register("scroll_offset", 0.0, 8.0);
animation_system.borrow_mut().register("tab_position", 0.0, 1.0);

// Update loop (60 FPS)
let anim_sys = animation_system.clone();
glib::timeout_add_local(Duration::from_millis(16), move || {
    let delta_time = 0.016; // 16ms = 60 FPS
    let values = anim_sys.borrow_mut().update(delta_time);
    
    // Apply values to widgets
    if let Some(&width) = values.get("column_width") {
        column.set_width_request(width as i32);
    }
    
    glib::ControlFlow::Continue
});

// Add toggle button in settings
let toggle_density = gtk::Button::with_label("Toggle Jiggle Animation");
let anim_sys_for_toggle = animation_system.clone();

toggle_density.connect_clicked(move |_| {
    anim_sys_for_toggle.borrow_mut().toggle_density_mode();
});
```

---

### 6. Snap Scroll with Inertial Animation

```rust
pub struct SnapScroller {
    scroll_offset: InertialLerp,
    snap_points: Vec<f32>,
    dragging: bool,
    drag_velocity: f32,
}

impl SnapScroller {
    pub fn new(snap_points: Vec<f32>) -> Self {
        Self {
            scroll_offset: InertialLerp::new(0.0, 5.0),
            snap_points,
            dragging: false,
            drag_velocity: 0.0,
        }
    }
    
    /// Handle scroll with inertia
    pub fn on_scroll_end(&mut self) {
        self.dragging = false;
        
        // Find nearest snap point
        let current = self.scroll_offset.current;
        
        // Add velocity to predict where user "wanted" to go
        let predicted = current + self.drag_velocity * 0.5;
        
        let target = find_nearest_snap_binary(predicted, &self.snap_points);
        
        self.scroll_offset.set_target(target);
    }
    
    /// Update (call every frame)
    pub fn update(&mut self, delta_time: f32) -> f32 {
        self.scroll_offset.update(delta_time)
    }
}
```

---

### 7. Settings UI for Density Toggle

```rust
// Add to settings page
let density_settings = gtk::Box::new(gtk::Orientation::Vertical, 10);

let density_label = gtk::Label::new(Some("Animation Style"));
density_label.add_css_class("setting-label");

let density_switch = gtk::Switch::new();
density_switch.set_active(false);  // Start with simple lerp

let anim_sys_for_switch = animation_system.clone();
density_switch.connect_state_set(move |_, enabled| {
    anim_sys_for_switch.borrow_mut().use_density_globally = enabled;
    
    for lerp in anim_sys_for_switch.borrow_mut().animations.values_mut() {
        lerp.set_use_density(enabled);
    }
    
    println!("🎭 Animation: {}", if enabled { "Jiggly" } else { "Smooth" });
    
    gtk::glib::Propagation::Proceed
});

let density_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
density_row.append(&density_label);
density_row.append(&density_switch);

density_settings.append(&density_row);
```

---

## 🎨 **Visual Examples**

### Simple Lerp (use_density = false):
```
Target changed from 0 → 100

Frame 1:  0 → 15  (smooth)
Frame 2: 15 → 28  (smooth)
Frame 3: 28 → 40  (smooth)
Frame 4: 40 → 51  (smooth)
...
Frame N: 98 → 100 (smooth stop)
```

### Density-based (use_density = true, mass = 5.0):
```
Target changed from 0 → 100

Frame 1:  0 →  2   (slow start - heavy)
Frame 2:  2 →  6   (accelerating)
Frame 3:  6 → 13   (picking up speed)
Frame 4: 13 → 24   (momentum building)
...
Frame 10: 95 → 103 (overshoot!)
Frame 11: 103 → 101 (bounce back)
Frame 12: 101 → 100 (settle with jiggle)
```

---

## 📊 **Performance Comparison**

| Algorithm | Time Complexity | Use Case | Memory |
|-----------|----------------|----------|--------|
| Linear scan | O(n) | <100 snap points | O(1) |
| Binary search | O(log n) | >100 snap points | O(1) |
| Dijkstra | O(E log V) | Graph paths | O(V+E) |
| MST | O(E log V) | Connecting points | O(V+E) |

**Recommendation:** Use linear scan (it's fast enough and simpler).

---

## 🎯 **Implementation Priority**

### Must Have (Phase 2-3):
1. ✅ Basic snap scrolling (linear scan)
2. ✅ Simple lerp animations
3. ✅ Viewport detection

### Nice to Have (Phase 4-5):
1. ⬜ Inertial animations with spring physics
2. ⬜ Density-based mass simulation
3. ⬜ Settings toggle for animation style

### Future (Phase 6+):
1. ⬜ Binary search optimization (if >100 snap points)
2. ⬜ Per-component mass configuration
3. ⬜ Animation presets (bouncy, smooth, snappy)

---

## 🎉 **Summary**

**Snap Scrolling:**
- Use simple linear scan (O(n))
- Binary search only if >100 snap points
- Spring physics for smooth animation
- **No Dijkstra/MST needed** (they're for graphs, not 1D distances)

**Inertial Animations:**
- ✅ Yes, definitely possible with lerp
- ✅ Density simulation via mass parameter
- ✅ Toggle in Settings → Appearance
- ✅ Different masses for different component types
- ✅ Spring physics creates natural jiggle

**User Control:**
```
Settings → Appearance
  [ ] Jiggly Animations (Spring Physics)
```

When enabled:
- Light components: Quick, snappy (mass = 1.0)
- Medium components: Balanced (mass = 3.0)
- Heavy components: Weighty, momentum (mass = 8.0)

**Lets implement Phase 1 first, then add these advanced features!** 🚀
