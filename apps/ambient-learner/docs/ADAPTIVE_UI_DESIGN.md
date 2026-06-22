# Adaptive Multi-Viewport UI Design

## 🎨 **Vision**

Create a fluid, context-aware interface that adapts to viewport size with intelligent content organization:

- **Desktop:** Multi-column layout with section labels, partial page reveals
- **Tablet:** Hybrid with collapsible sections
- **Mobile:** Infinite scroll with sticky category headers and dynamic tab highlighting

---

## 📐 **Layout Modes**

### Desktop Mode (≥1200px)
```
┌─────────────────────────────────────────────────────┐
│ [Live] [Model] [History] [Locations] [Sync]        │ ← Tabs (static)
├──────────────┬──────────────┬───────────────────────┤
│              │              │                       │
│   LIVE       │   MODEL      │   HISTORY             │
│   (Primary)  │   (Partial)  │   (Peek)             │
│              │              │                       │
│   25.3°C     │   Neural     │   ┌─────┐            │
│   ████       │   Field      │   │Chart│            │
│   Confidence │   ●●●        │   └─────┘            │
│              │              │                       │
└──────────────┴──────────────┴───────────────────────┘
     60%            30%             10%
```

**Features:**
- Primary content (60%) + partial sections (30% + 10%)
- Section labels sticky at top
- Smooth horizontal transitions
- Drag to resize columns
- Click section label to expand full-width

---

### Tablet Mode (768px - 1199px)
```
┌─────────────────────────────────┐
│ [Live] [Model] [History] ...    │ ← Scrollable tabs
├─────────────────────────────────┤
│                                 │
│   LIVE                          │
│   (Expanded)                    │
│                                 │
│   25.3°C                        │
│   ████████                      │
│   Confidence: 85%               │
│                                 │
├─────────────────────────────────┤
│ MODEL ▼                         │ ← Collapsible
├─────────────────────────────────┤
│   Neural Field Preview          │
│   ●●●●                          │
└─────────────────────────────────┘
```

**Features:**
- Accordion-style sections
- Swipe between tabs
- Tap headers to expand/collapse
- Smooth animations

---

### Mobile Mode (≤767px)
```
┌──────────────┐
│ Live • Model │ ← Multi-highlight tabs
├──────────────┤
│              │
│   LIVE       │ ← Sticky header
│              │
│   25.3°C     │
│   ████       │
│              │
│ ─ ─ ─ ─ ─ ─  │ ← Scroll indicator
│              │
│   MODEL      │ ← Sticky header
│              │
│   Neural     │
│   Field      │
│              │
│ ─ ─ ─ ─ ─ ─  │
│              │
│   HISTORY    │
│              │
└──────────────┘
```

**Features:**
- Infinite scroll with snap points
- Section headers stick during scroll
- Tabs highlight based on visible sections
- Multiple tabs highlighted if sections overlap
- Pull-to-refresh
- Bottom nav for quick jumps

---

## 🏗️ **Architecture**

### Component Hierarchy
```
AdaptiveLayout
├── ViewportDetector (hook)
├── LayoutController (state machine)
├── DesktopLayout
│   ├── MultiColumnView
│   │   ├── SectionColumn (primary)
│   │   ├── SectionColumn (partial)
│   │   └── SectionColumn (peek)
│   └── ResizeHandles
├── TabletLayout
│   ├── AccordionView
│   │   ├── Section (collapsible)
│   │   └── Section (collapsible)
│   └── SwipeHandler
└── MobileLayout
    ├── InfiniteScrollView
    │   ├── StickyHeader
    │   ├── Section
    │   └── ScrollProgress
    └── MultiHighlightTabs
```

---

## 🎯 **Core Features**

### 1. Viewport Detection
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewportMode {
    Desktop,   // ≥1200px
    Tablet,    // 768-1199px
    Mobile,    // <768px
}

pub struct ViewportDetector {
    width: f32,
    height: f32,
    mode: ViewportMode,
}

impl ViewportDetector {
    pub fn detect_mode(width: f32) -> ViewportMode {
        match width {
            w if w >= 1200.0 => ViewportMode::Desktop,
            w if w >= 768.0 => ViewportMode::Tablet,
            _ => ViewportMode::Mobile,
        }
    }
    
    pub fn calculate_layout(&self, sections: &[Section]) -> Layout {
        match self.mode {
            ViewportMode::Desktop => {
                self.desktop_layout(sections)
            }
            ViewportMode::Tablet => {
                self.tablet_layout(sections)
            }
            ViewportMode::Mobile => {
                self.mobile_layout(sections)
            }
        }
    }
}
```

---

### 2. Desktop Multi-Column Layout

```rust
pub struct DesktopLayout {
    columns: Vec<ColumnConfig>,
    total_width: f32,
}

pub struct ColumnConfig {
    section: Section,
    width_percent: f32,
    min_width: f32,
    state: ColumnState,
}

pub enum ColumnState {
    Primary,    // Full detail
    Partial,    // Key info only
    Peek,       // Minimized preview
}

impl DesktopLayout {
    pub fn auto_layout(sections: &[Section], width: f32) -> Self {
        // Primary section gets 60%
        let mut columns = vec![
            ColumnConfig {
                section: sections[0].clone(),
                width_percent: 60.0,
                min_width: 400.0,
                state: ColumnState::Primary,
            }
        ];
        
        // Remaining width distributed
        let remaining = 40.0;
        let available_sections = &sections[1..];
        
        // Fit as many sections as possible
        let mut used = 0.0;
        for section in available_sections {
            let min_needed = section.min_width / width * 100.0;
            
            if used + min_needed <= remaining {
                columns.push(ColumnConfig {
                    section: section.clone(),
                    width_percent: min_needed,
                    min_width: section.min_width,
                    state: if used + min_needed < 20.0 {
                        ColumnState::Peek
                    } else {
                        ColumnState::Partial
                    },
                });
                used += min_needed;
            } else {
                break;
            }
        }
        
        Self { columns, total_width: width }
    }
}
```

---

### 3. Section Labels & Sticky Headers

```rust
pub struct StickyHeader {
    section: Section,
    y_offset: f32,
    is_stuck: bool,
}

impl StickyHeader {
    pub fn update(&mut self, scroll_y: f32, section_top: f32) {
        self.is_stuck = scroll_y >= section_top;
        
        if self.is_stuck {
            self.y_offset = 0.0; // Stick to top
        } else {
            self.y_offset = section_top - scroll_y;
        }
    }
    
    pub fn render(&self) -> Element {
        container(
            text(self.section.name())
                .size(14)
                .weight(Bold)
        )
        .padding(10)
        .style(if self.is_stuck {
            StickyHeaderStyle::Stuck
        } else {
            StickyHeaderStyle::Normal
        })
    }
}
```

---

### 4. Mobile Infinite Scroll with Multi-Highlight

```rust
pub struct MobileScrollView {
    sections: Vec<Section>,
    scroll_offset: f32,
    visible_sections: Vec<usize>,
}

impl MobileScrollView {
    pub fn detect_visible_sections(
        &self, 
        scroll_y: f32, 
        viewport_height: f32
    ) -> Vec<usize> {
        let mut visible = Vec::new();
        
        for (idx, section) in self.sections.iter().enumerate() {
            let section_top = section.y_offset;
            let section_bottom = section_top + section.height;
            
            // Section is visible if any part is in viewport
            let viewport_top = scroll_y;
            let viewport_bottom = scroll_y + viewport_height;
            
            let is_visible = 
                (section_top >= viewport_top && section_top <= viewport_bottom) ||
                (section_bottom >= viewport_top && section_bottom <= viewport_bottom) ||
                (section_top <= viewport_top && section_bottom >= viewport_bottom);
            
            if is_visible {
                visible.push(idx);
            }
        }
        
        visible
    }
    
    pub fn calculate_highlight_weights(&self) -> Vec<(usize, f32)> {
        // Calculate how much of each section is visible
        let mut weights = Vec::new();
        
        for &idx in &self.visible_sections {
            let section = &self.sections[idx];
            let visible_height = self.calculate_visible_height(section);
            let weight = visible_height / section.height;
            weights.push((idx, weight));
        }
        
        weights
    }
}
```

---

### 5. Multi-Highlight Tabs

```rust
pub struct MultiHighlightTabs {
    tabs: Vec<Tab>,
    highlights: Vec<(usize, f32)>, // (tab_index, weight 0.0-1.0)
}

impl MultiHighlightTabs {
    pub fn render(&self) -> Element {
        row(
            self.tabs.iter().enumerate().map(|(idx, tab)| {
                let weight = self.highlights
                    .iter()
                    .find(|(i, _)| *i == idx)
                    .map(|(_, w)| *w)
                    .unwrap_or(0.0);
                
                self.render_tab(tab, weight)
            })
        )
    }
    
    fn render_tab(&self, tab: &Tab, weight: f32) -> Element {
        let alpha = (weight * 255.0) as u8;
        let bg_color = Color::from_rgba(
            0.2, 0.6, 1.0, 
            weight  // Opacity based on visibility
        );
        
        button(text(tab.name))
            .style(TabStyle {
                background: bg_color,
                border_width: if weight > 0.5 { 2.0 } else { 0.0 },
            })
    }
}
```

---

## 🎭 **Animation System**

### Smooth Transitions

```rust
pub struct LayoutTransition {
    from: Layout,
    to: Layout,
    progress: f32,  // 0.0 to 1.0
    duration: Duration,
}

impl LayoutTransition {
    pub fn animate(&mut self, delta_time: Duration) -> Layout {
        self.progress += delta_time.as_secs_f32() / self.duration.as_secs_f32();
        self.progress = self.progress.min(1.0);
        
        // Easing function (ease-out cubic)
        let eased = 1.0 - (1.0 - self.progress).powi(3);
        
        // Interpolate layout
        Layout {
            columns: self.from.columns.iter()
                .zip(&self.to.columns)
                .map(|(from, to)| {
                    ColumnConfig {
                        width_percent: lerp(
                            from.width_percent, 
                            to.width_percent, 
                            eased
                        ),
                        ..from.clone()
                    }
                })
                .collect()
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
```

---

### Scroll Snap Points

```rust
pub struct SnapScroll {
    snap_points: Vec<f32>,
    velocity: f32,
    target_offset: f32,
}

impl SnapScroll {
    pub fn find_nearest_snap(&self, current_offset: f32) -> f32 {
        self.snap_points
            .iter()
            .min_by(|a, b| {
                let dist_a = (current_offset - **a).abs();
                let dist_b = (current_offset - **b).abs();
                dist_a.partial_cmp(&dist_b).unwrap()
            })
            .copied()
            .unwrap_or(current_offset)
    }
    
    pub fn animate_to_snap(&mut self, delta_time: Duration) -> f32 {
        let diff = self.target_offset - self.current_offset;
        let step = diff * 0.15; // Spring constant
        
        self.current_offset += step;
        
        // Stop if close enough
        if diff.abs() < 0.5 {
            self.current_offset = self.target_offset;
        }
        
        self.current_offset
    }
}
```

---

## 📱 **GTK Implementation**

### Responsive Stack

```rust
// In main.rs, replace fixed stack with responsive layout

let stack = gtk::Stack::new();
let stack_switcher = gtk::StackSwitcher::new();

// Detect window size changes
window.connect_default_width_notify(move |window| {
    let width = window.default_width();
    let mode = ViewportDetector::detect_mode(width as f32);
    
    match mode {
        ViewportMode::Desktop => {
            // Show multi-column layout
            stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        }
        ViewportMode::Tablet => {
            // Show accordion layout
            stack.set_transition_type(gtk::StackTransitionType::SlideUpDown);
        }
        ViewportMode::Mobile => {
            // Show scroll layout
            stack.set_transition_type(gtk::StackTransitionType::None);
        }
    }
});
```

---

### Desktop Multi-Column with Box

```rust
fn create_desktop_layout() -> gtk::Box {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    
    // Primary column (60%)
    let primary_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    primary_box.set_hexpand(true);
    primary_box.set_width_request(400);
    
    // Section header (sticky)
    let header = gtk::Label::new(Some("LIVE"));
    header.add_css_class("section-header");
    primary_box.append(&header);
    
    // Content
    let content = create_live_content();
    primary_box.append(&content);
    
    hbox.append(&primary_box);
    
    // Separator
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    hbox.append(&separator);
    
    // Partial column (30%)
    let partial_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    partial_box.set_hexpand(true);
    partial_box.set_width_request(300);
    
    let header2 = gtk::Label::new(Some("MODEL"));
    header2.add_css_class("section-header");
    partial_box.append(&header2);
    
    let content2 = create_model_preview();
    partial_box.append(&content2);
    
    hbox.append(&partial_box);
    
    hbox
}
```

---

### Mobile Infinite Scroll with ScrolledWindow

```rust
fn create_mobile_layout() -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    // Multi-highlight tabs at top
    let tabs = create_multi_highlight_tabs();
    vbox.append(&tabs);
    
    // Scrolled window
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    
    let scroll_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    // Add all sections
    for section in ["Live", "Model", "History", "Locations", "Sync"] {
        // Sticky header
        let header = gtk::Label::new(Some(section));
        header.add_css_class("sticky-header");
        scroll_box.append(&header);
        
        // Content
        let content = create_section_content(section);
        scroll_box.append(&content);
    }
    
    scrolled.set_child(Some(&scroll_box));
    
    // Track scroll position for tab highlighting
    let adjustment = scrolled.vadjustment();
    adjustment.connect_value_changed(move |adj| {
        let scroll_y = adj.value();
        update_tab_highlights(scroll_y);
    });
    
    vbox.append(&scrolled);
    vbox
}
```

---

## 🎨 **CSS Styling**

```css
/* Desktop section headers */
.section-header {
    font-size: 12px;
    font-weight: bold;
    color: rgba(255, 255, 255, 0.6);
    padding: 10px;
    background: rgba(0, 0, 0, 0.2);
}

/* Sticky headers (mobile) */
.sticky-header {
    font-size: 14px;
    font-weight: bold;
    padding: 15px;
    background: rgba(0, 0, 0, 0.95);
    border-bottom: 1px solid rgba(255, 255, 255, 0.1);
    position: sticky;
    top: 0;
    z-index: 100;
}

/* Multi-highlight tabs */
.multi-highlight-tab {
    transition: background-color 0.3s ease, opacity 0.3s ease;
    padding: 10px 20px;
    border-radius: 5px 5px 0 0;
}

.multi-highlight-tab.active {
    background: rgba(33, 150, 243, 0.8);
    border-bottom: 3px solid #2196F3;
}

.multi-highlight-tab.partial {
    background: rgba(33, 150, 243, 0.4);
    border-bottom: 2px solid rgba(33, 150, 243, 0.6);
}

/* Column resize handle */
.resize-handle {
    width: 4px;
    background: rgba(255, 255, 255, 0.1);
    cursor: col-resize;
}

.resize-handle:hover {
    background: rgba(33, 150, 243, 0.5);
}

/* Smooth transitions */
.layout-transition {
    transition: all 0.5s cubic-bezier(0.25, 0.46, 0.45, 0.94);
}
```

---

## 🎯 **Implementation Phases**

### Phase 1: Viewport Detection (2 hours)
- [ ] Add window size tracking
- [ ] Implement ViewportMode enum
- [ ] Create mode switcher
- [ ] Test resize behavior

### Phase 2: Desktop Multi-Column (4 hours)
- [ ] Create column layout engine
- [ ] Add resize handles
- [ ] Implement section labels
- [ ] Add smooth transitions
- [ ] Test with different widths

### Phase 3: Tablet Accordion (3 hours)
- [ ] Create collapsible sections
- [ ] Add swipe gestures
- [ ] Implement animations
- [ ] Test on tablet viewport

### Phase 4: Mobile Infinite Scroll (4 hours)
- [ ] Create scroll container
- [ ] Add sticky headers
- [ ] Implement snap points
- [ ] Add pull-to-refresh
- [ ] Test smooth scrolling

### Phase 5: Multi-Highlight Tabs (3 hours)
- [ ] Create tab component
- [ ] Implement visibility detection
- [ ] Add weight calculation
- [ ] Animate highlights
- [ ] Test with scroll

### Phase 6: Polish & Optimization (2 hours)
- [ ] Fine-tune animations
- [ ] Optimize scroll performance
- [ ] Add loading states
- [ ] Test edge cases
- [ ] Performance profiling

**Total: ~18 hours**

---

## 📊 **Examples**

### Desktop (1920x1080)
```
┌──────────────────────┬────────────┬──────┐
│ LIVE (60%)           │ MODEL (30%)│HIST  │
│                      │            │(10%) │
│ 25.3°C               │ Neural     │Chart │
│ ██████████           │ Field      │●●    │
│ Confidence: 85%      │ ●●●●●      │      │
│                      │ ●●●●●      │      │
│ Sensors: 53          │ ●●●●●      │      │
│ Model: Neural        │            │      │
│ Calibrations: 12     │ Trained    │      │
└──────────────────────┴────────────┴──────┘
← Drag handles to resize columns →
```

### Tablet (768x1024)
```
┌─────────────────────────┐
│ LIVE ▲                  │
├─────────────────────────┤
│ 25.3°C                  │
│ ██████████              │
│ Confidence: 85%         │
├─────────────────────────┤
│ MODEL ▼                 │
├─────────────────────────┤
│ (Collapsed - tap to     │
│  expand)                │
├─────────────────────────┤
│ HISTORY ▼               │
└─────────────────────────┘
```

### Mobile (375x812)
```
┌──────────────┐
│ Live•Model   │ ← Multiple active
├──────────────┤
│  LIVE        │ ← Sticky
│              │
│  25.3°C      │
│  ████        │
│              │
│ ─────────    │ ← 40% scrolled
│              │
│  MODEL       │ ← Sticky
│              │
│  Neural      │
│              │
└──────────────┘
```

---

## 🎉 **Summary**

**What This Achieves:**

**Desktop:**
- ✅ See multiple sections at once
- ✅ Drag to resize
- ✅ Click labels to focus
- ✅ Efficient use of space

**Tablet:**
- ✅ Expandable sections
- ✅ Swipe navigation
- ✅ Touch-optimized

**Mobile:**
- ✅ Natural scrolling
- ✅ Context awareness (multi-highlight)
- ✅ Sticky headers
- ✅ Snap scrolling

**Result:** One codebase, three optimized experiences! 🚀
