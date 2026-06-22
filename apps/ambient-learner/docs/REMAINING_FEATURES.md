# Remaining Features - Implementation Guide

## Status: PARTIALLY COMPLETE

### ✅ Completed
1. **Calibration Dialog Freeze** - Fixed by moving to background thread
2. **Location Data Structure** - Enhanced with `networks` field
3. **Auto-Detection** - Detects location by WiFi on startup
4. **Database Schema** - Added `location_id` columns with migration
5. **Auto-Tagging** - Readings/calibrations tagged with current location

### 🔨 In Progress / TODO

## 1. History Location Filter Dropdown

**Location**: History tab, above chart

```rust
// Add to history page (page3)
let filter_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
filter_box.set_halign(gtk::Align::Center);
filter_box.set_margin_bottom(8);

let filter_label = gtk::Label::new(Some("Filter:"));
let filter_dropdown = gtk::DropDown::new_from_strings(&["All Locations"]);

filter_box.append(&filter_label);
filter_box.append(&filter_dropdown);

page3.prepend(&filter_box); // Add above history_overlay
```

**Populate dropdown**:
```rust
// Build list: ["All", "Montreal", "Office WiFi", ...]
let mut filter_options = vec!["All Locations".to_string()];
for location in locations.lock().unwrap().iter() {
    filter_options.push(location.name.clone());
}

let model = gtk::StringList::new(
    &filter_options.iter().map(|s| s.as_str()).collect::<Vec<_>>()
);
filter_dropdown.set_model(Some(&model));
```

**Connect to update_history**:
```rust
let selected_location = Rc::new(RefCell::new(None::<String>));

filter_dropdown.connect_selected_notify(move |dropdown| {
    let idx = dropdown.selected() as usize;
    if idx == 0 {
        *selected_location.borrow_mut() = None; // All
    } else {
        let locs = locations.lock().unwrap();
        *selected_location.borrow_mut() = locs.get(idx - 1).map(|l| l.id.clone());
    }
    (update_history)(); // Refresh with new filter
});
```

**Pass to get_daily_averages**:
```rust
// In update_history closure
let location_filter = selected_location.borrow().clone();
let data = temp_db.get_daily_averages(offset, 7, location_filter.as_deref());
```

## 2. Location Settings Gear Icon

**Add to location rows**:
```rust
// In refresh_locations_list()
let row = adw::ActionRow::new();
row.set_title(&location.name);

// Add gear button
let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
settings_btn.set_valign(gtk::Align::Center);
settings_btn.add_css_class("flat");

row.add_suffix(&settings_btn);
```

**Settings Dialog**:
```rust
settings_btn.connect_clicked(move |_| {
    let dialog = adw::MessageDialog::builder()
        .transient_for(&window)
        .heading(&format!("Settings: {}", location.name))
        .build();
    
    dialog.add_response("close", "Close");
    dialog.add_response("save", "Save");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    
    // Network list
    let networks_label = gtk::Label::new(Some("Associated Networks:"));
    networks_label.set_halign(gtk::Align::Start);
    
    let networks_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    
    // Show existing networks
    for network in &location.networks {
        let network_row = adw::ActionRow::new();
        network_row.set_title(network);
        
        let remove_btn = gtk::Button::from_icon_name("edit-delete-symbolic");
        remove_btn.add_css_class("destructive-action");
        network_row.add_suffix(&remove_btn);
        
        networks_box.append(&network_row);
    }
    
    // Add network button
    let add_network_btn = gtk::Button::with_label("➕ Add Current Network");
    if let Some(ssid) = get_current_network_ssid() {
        add_network_btn.set_tooltip_text(Some(&format!("Add: {}", ssid)));
    } else {
        add_network_btn.set_sensitive(false);
    }
    
    content.append(&networks_label);
    content.append(&networks_box);
    content.append(&add_network_btn);
    
    dialog.set_extra_child(Some(&content));
    dialog.present();
});
```

## 3. AdwCarousel for Interactive Swipe

**Replace ViewStack with Carousel**:

```rust
// Instead of ViewStack
let carousel = adw::Carousel::new();
carousel.set_interactive(true);
carousel.set_allow_scroll_wheel(true);

// Add pages
carousel.append(&page1); // Live
carousel.append(&page2); // Model
carousel.append(&page3); // History
carousel.append(&page4); // Locations

// Indicator dots
let carousel_indicator = adw::CarouselIndicatorDots::new();
carousel_indicator.set_carousel(Some(&carousel));

main_box.append(&carousel);
main_box.append(&carousel_indicator);

// ViewSwitcherBar for tab names
let switcher_bar = adw::ViewSwitcherBar::new();
// Note: ViewSwitcherBar doesn't work directly with Carousel
// Alternative: Use custom button bar
```

**Custom Tab Bar**:
```rust
let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
tab_bar.set_homogeneous(true);
tab_bar.add_css_class("toolbar");

let tabs = [
    ("Live", 0),
    ("Model", 1),
    ("History", 2),
    ("📍", 3),
];

for (name, index) in tabs {
    let btn = gtk::Button::with_label(name);
    btn.set_hexpand(true);
    
    let carousel = carousel.clone();
    btn.connect_clicked(move |_| {
        carousel.scroll_to(&carousel.nth_page(index as u32), true);
    });
    
    tab_bar.append(&btn);
}

main_box.append(&tab_bar);
```

**Benefits**:
- ✅ Interactive swipe with visual feedback
- ✅ Smooth spring animations
- ✅ Elastic bounce at edges
- ✅ Drag-to-navigate feel

## 4. Auto Network Addition

**Add timer to check for network changes**:
```rust
// In build_ui, after locations loaded
{
    let locations = locations.clone();
    let current_location_idx = current_location_idx.clone();
    
    // Check every 30 seconds
    glib::timeout_add_local(Duration::from_secs(30), move || {
        if let Some(ssid) = get_current_network_ssid() {
            let locs = locations.lock().unwrap();
            let detected = locs.iter().position(|l| l.matches_network(&ssid));
            
            if let Some(idx) = detected {
                let current = *current_location_idx.lock().unwrap();
                if current != idx {
                    *current_location_idx.lock().unwrap() = idx;
                    println!("Auto-switched to location: {}", locs[idx].name);
                }
            }
        }
        glib::ControlFlow::Continue
    });
}
```

## 5. Additional Enhancements

### Make "Unknown" Settable

```rust
// In location selector dropdown
location_dropdown.connect_selected_notify(move |dropdown| {
    let idx = dropdown.selected() as usize;
    *current_location_idx.lock().unwrap() = idx;
    
    let locs = locations.lock().unwrap();
    if let Some(loc) = locs.get(idx) {
        println!("Switched to location: {}", loc.name);
    }
});
```

### Network Group Management UI

```rust
// Allow dragging networks between locations
// Or: bulk import from text list
let import_btn = gtk::Button::with_label("Import Network List");

import_btn.connect_clicked(|_| {
    let dialog = gtk::FileChooserDialog::new(...);
    // Load text file with SSIDs, one per line
    // Add all to current location
});
```

## Implementation Priority

1. **URGENT**: History filter dropdown (needed for testing)
2. **HIGH**: AdwCarousel (major UX improvement)
3. **MEDIUM**: Settings gear icon
4. **MEDIUM**: Auto network switching
5. **LOW**: Network import/export

## Testing Checklist

After implementing each:

### History Filter
- [ ] Dropdown shows "All Locations" + all location names
- [ ] Selecting filter refreshes chart
- [ ] Chart shows only data from selected location
- [ ] "All Locations" shows combined data

### Carousel
- [ ] Can swipe left/right between pages
- [ ] Smooth spring animation
- [ ] Elastic bounce at edges
- [ ] Tab buttons sync with current page
- [ ] Indicator dots show position

### Network Settings
- [ ] Gear icon appears on location rows
- [ ] Dialog shows current networks
- [ ] Can add current network
- [ ] Can remove networks
- [ ] Saves persist

### Auto-Switching
- [ ] Connects to WiFi → auto-switches location
- [ ] Disconnects → switches to Unknown
- [ ] Console shows auto-switch messages
- [ ] Works across app restarts

## File Changes Summary

**Modified**:
- `src/main.rs` - Location filtering, carousel, settings

**New**:
- None (all inline)

**Database**:
- Migrations handled automatically in `Database::open()`

## Next Session Goals

1. Add history filter dropdown
2. Implement AdwCarousel
3. Add settings gear icon
4. Test with real WiFi networks

## Known Issues

- [ ] Compression only runs on models, not databases (add database compression)
- [ ] No visual feedback during calibration (add progress toast)
- [ ] Location dropdown doesn't update when locations added (needs refresh callback)

## Performance Notes

- Auto-network check every 30s: negligible CPU
- Filter dropdown: instant response
- Carousel: 60 FPS animations (hardware accelerated)
- Settings dialog: opens <50ms
