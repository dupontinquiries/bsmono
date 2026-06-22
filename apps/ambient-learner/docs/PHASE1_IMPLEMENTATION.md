# Phase 1: Floating Titles & Viewport Detection - Ready to Paste

## 🚀 **Implementation Now - Copy & Paste Ready**

### Step 1: Add CSS for Floating Titles

**PASTE THIS into `src/style.css` at the end:**

```css
/* ============================================ */
/* Phase 1: Floating Titles & Dynamic Colors   */
/* ============================================ */

/* Floating section titles */
.section-title-float {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 1.2px;
    text-transform: uppercase;
    padding: 8px 16px;
    margin: 15px;
    border-radius: 8px;
    
    /* Floating effect */
    background: rgba(0, 0, 0, 0.7);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
    
    /* Drop shadow */
    box-shadow: 
        0 4px 6px rgba(0, 0, 0, 0.4),
        0 1px 3px rgba(0, 0, 0, 0.3),
        inset 0 1px 0 rgba(255, 255, 255, 0.1);
}

/* Dynamic color blur spill for each section */
.section-live {
    background: linear-gradient(135deg, 
        rgba(33, 150, 243, 0.05) 0%, 
        rgba(33, 150, 243, 0.02) 100%);
    border-left: 3px solid rgba(33, 150, 243, 0.5);
}

.section-live .section-title-float {
    color: rgba(100, 181, 246, 1);
    box-shadow: 
        0 4px 12px rgba(33, 150, 243, 0.6),
        0 2px 6px rgba(33, 150, 243, 0.4),
        inset 0 1px 0 rgba(100, 181, 246, 0.2);
}

.section-model {
    background: linear-gradient(135deg, 
        rgba(156, 39, 176, 0.05) 0%, 
        rgba(156, 39, 176, 0.02) 100%);
    border-left: 3px solid rgba(156, 39, 176, 0.5);
}

.section-model .section-title-float {
    color: rgba(186, 104, 200, 1);
    box-shadow: 
        0 4px 12px rgba(156, 39, 176, 0.6),
        0 2px 6px rgba(156, 39, 176, 0.4),
        inset 0 1px 0 rgba(186, 104, 200, 0.2);
}

.section-history {
    background: linear-gradient(135deg, 
        rgba(255, 152, 0, 0.05) 0%, 
        rgba(255, 152, 0, 0.02) 100%);
    border-left: 3px solid rgba(255, 152, 0, 0.5);
}

.section-history .section-title-float {
    color: rgba(255, 183, 77, 1);
    box-shadow: 
        0 4px 12px rgba(255, 152, 0, 0.6),
        0 2px 6px rgba(255, 152, 0, 0.4),
        inset 0 1px 0 rgba(255, 183, 77, 0.2);
}

.section-locations {
    background: linear-gradient(135deg, 
        rgba(76, 175, 80, 0.05) 0%, 
        rgba(76, 175, 80, 0.02) 100%);
    border-left: 3px solid rgba(76, 175, 80, 0.5);
}

.section-locations .section-title-float {
    color: rgba(129, 199, 132, 1);
    box-shadow: 
        0 4px 12px rgba(76, 175, 80, 0.6),
        0 2px 6px rgba(76, 175, 80, 0.4),
        inset 0 1px 0 rgba(129, 199, 132, 0.2);
}

.section-sync {
    background: linear-gradient(135deg, 
        rgba(244, 67, 54, 0.05) 0%, 
        rgba(244, 67, 54, 0.02) 100%);
    border-left: 3px solid rgba(244, 67, 54, 0.5);
}

.section-sync .section-title-float {
    color: rgba(239, 83, 80, 1);
    box-shadow: 
        0 4px 12px rgba(244, 67, 54, 0.6),
        0 2px 6px rgba(244, 67, 54, 0.4),
        inset 0 1px 0 rgba(239, 83, 80, 0.2);
}

/* Column separator with blur */
.column-separator {
    background: linear-gradient(
        to bottom,
        rgba(255, 255, 255, 0.05),
        rgba(255, 255, 255, 0.15),
        rgba(255, 255, 255, 0.05)
    );
    min-width: 1px;
    margin: 20px 0;
}
```

---

### Step 2: Add Viewport Detector to main.rs

**PASTE THIS after your imports (around line 65):**

```rust
// ============================================================================
// Phase 1: Viewport Detection & Responsive Layout
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewportMode {
    Desktop,   // ≥1200px
    Tablet,    // 768-1199px  
    Mobile,    // <768px
}

impl ViewportMode {
    fn from_width(width: i32) -> Self {
        match width {
            w if w >= 1200 => ViewportMode::Desktop,
            w if w >= 768 => ViewportMode::Tablet,
            _ => ViewportMode::Mobile,
        }
    }
    
    fn as_str(&self) -> &'static str {
        match self {
            ViewportMode::Desktop => "desktop",
            ViewportMode::Tablet => "tablet",
            ViewportMode::Mobile => "mobile",
        }
    }
}
```

---

### Step 3: Add Wrapper Function

**PASTE THIS before run_gtk_ui() function:**

```rust
/// Wrap a page widget with a floating title and dynamic color
fn wrap_with_floating_title(
    content: gtk::Widget,
    section_name: &str,
    section_class: &str,
) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.add_css_class(section_class);
    vbox.set_vexpand(true);
    vbox.set_hexpand(true);
    
    // Floating title
    let title = gtk::Label::new(Some(section_name));
    title.add_css_class("section-title-float");
    title.set_halign(gtk::Align::Start);
    
    vbox.append(&title);
    vbox.append(&content);
    
    vbox
}
```

---

### Step 4: Update Stack Pages

**FIND THIS in run_gtk_ui():**

```rust
stack.add_titled(&page1, Some("page1"), "Live");
stack.add_titled(&page2, Some("page2"), "Model");
stack.add_titled(&page3, Some("page3"), "History");
stack.add_titled(&page4, Some("page4"), "Locations");
stack.add_titled(&page5, Some("page5"), "Sync");
```

**REPLACE WITH:**

```rust
// Phase 1: Wrap pages with floating titles and dynamic colors
let page1_wrapped = wrap_with_floating_title(page1, "LIVE", "section-live");
let page2_wrapped = wrap_with_floating_title(page2, "MODEL", "section-model");
let page3_wrapped = wrap_with_floating_title(page3, "HISTORY", "section-history");
let page4_wrapped = wrap_with_floating_title(page4, "LOCATIONS", "section-locations");
let page5_wrapped = wrap_with_floating_title(page5, "SYNC", "section-sync");

stack.add_titled(&page1_wrapped, Some("page1"), "Live");
stack.add_titled(&page2_wrapped, Some("page2"), "Model");
stack.add_titled(&page3_wrapped, Some("page3"), "History");
stack.add_titled(&page4_wrapped, Some("page4"), "Locations");
stack.add_titled(&page5_wrapped, Some("page5"), "Sync");
```

---

### Step 5: Add Viewport Detection

**FIND THIS in run_gtk_ui():**

```rust
window.set_default_size(800, 600);
```

**ADD AFTER IT:**

```rust
// Phase 1: Viewport detection
let current_mode = Rc::new(RefCell::new(ViewportMode::Desktop));
let mode_for_resize = current_mode.clone();

window.connect_default_width_notify(move |window| {
    let width = window.default_width();
    let new_mode = ViewportMode::from_width(width);
    let old_mode = *mode_for_resize.borrow();
    
    if old_mode != new_mode {
        println!("📱 Viewport mode: {} → {} ({}px)", 
            old_mode.as_str(), 
            new_mode.as_str(),
            width
        );
        *mode_for_resize.borrow_mut() = new_mode;
        
        // TODO Phase 2: Switch layouts here
    }
});

// Debug: Press 1/2/3 to test layouts
#[cfg(debug_assertions)]
{
    let window_clone = window.clone();
    let event_controller = gtk::EventControllerKey::new();
    
    event_controller.connect_key_pressed(move |_, keyval, _, _| {
        use gtk::gdk::Key;
        
        match keyval {
            Key::_1 => {
                window_clone.set_default_size(375, 812);
                println!("🎮 Testing: Mobile (375x812)");
                gtk::glib::Propagation::Stop
            }
            Key::_2 => {
                window_clone.set_default_size(768, 1024);
                println!("🎮 Testing: Tablet (768x1024)");
                gtk::glib::Propagation::Stop
            }
            Key::_3 => {
                window_clone.set_default_size(1920, 1080);
                println!("🎮 Testing: Desktop (1920x1080)");
                gtk::glib::Propagation::Stop
            }
            _ => gtk::glib::Propagation::Proceed,
        }
    });
    
    window.add_controller(event_controller);
    println!("🎮 Debug mode: Press 1/2/3 to test viewport sizes");
}
```

---

## 🧪 **Test Phase 1**

```bash
cargo build
./target/debug/ambient-learner
```

**Expected:**
- Floating titles on each page (LIVE, MODEL, etc.)
- Blue glow around Live section
- Purple glow around Model section
- Console shows viewport mode changes when resizing
- Press 1/2/3 to test different viewport sizes

---

## ✅ **Checklist**

- [ ] CSS pasted into `style.css`
- [ ] ViewportMode enum added after imports
- [ ] wrap_with_floating_title() function added
- [ ] Stack pages wrapped with titles
- [ ] Viewport detection added after window.set_default_size()
- [ ] App builds without errors
- [ ] Floating titles visible
- [ ] Color blur spill visible
- [ ] Debug keys work (1/2/3)

---

**Phase 1 Complete! Ready for Phase 2: Multi-Column Layout** 🚀
