# Practical Improvements Implementation Plan

## Phase 1: Location Management (IMPLEMENTING NOW)

### 1.1 Location Data Structures ✅
- [x] Location struct with ID, name, type
- [x] LocationType enum (City, NetworkId, Unknown)
- [x] UUID generation for location IDs
- [x] Serialization support

### 1.2 Location Helper Functions ✅
- [x] `get_timezone_cities()` - Extract cities from system timezones
- [x] `get_current_network_ssid()` - Detect current WiFi network
- [x] `save_locations()` - Persist to JSON
- [x] `load_locations()` - Load from JSON

### 1.3 Location UI (TODO)
- [ ] Add 4th tab "Locations"  
- [ ] Location list view
- [ ] Add location dialog with:
  - City name entry with dropdown suggestions
  - "Pin to Network" button
- [ ] Location selector in bottom bar
- [ ] Current location indicator

## Phase 2: UX Improvements

### 2.1 Page 1 Centering ✅
- [x] Reduced margins
- [x] Fill mode instead of center
- [ ] TODO: Dynamic padding based on available space

### 2.2 GTK Swipe Gestures ✅ (History only)
- [x] History page swipe working
- [ ] TODO: Add swipe between main pages (Live/Model/History/Locations)

### 2.3 Loading Indicators ✅
- [x] Spinner on history chart
- [ ] TODO: Spinner on initial load
- [ ] TODO: Button press feedback

## Phase 3: Database Optimization

### 3.1 Model File Compression
**Goal**: Reduce from ~1,440 model files/day to ~12/year

**Strategy**:
```rust
struct CompressedModel {
    month: String,  // YYYY-MM
    models: Vec<Model>,  // All models from that month
    compressed_data: Vec<u8>,  // Gzip compressed
}
```

**Implementation**:
- Keep latest uncompressed model for quick access
- Compress all models > 24 hours old by month
- Monthly model file: `models_2024-03.json.gz`
- Auto-cleanup: Delete individual files after compression

**Space Savings**:
- Before: 1,440 files/day × 5KB = 7.2 MB/day
- After: 1 file/month × 150KB = 150 KB/month  
- **Reduction**: 98% less disk usage

### 3.2 Database File Compression
**Goal**: Reduce database file proliferation

**Strategy**:
```rust
fn compress_old_databases() {
    // Find databases > 7 days old
    let old_dbs = find_databases_older_than(Duration::days(7));
    
    for db in old_dbs {
        // Export to compressed JSON
        let data = export_database_to_json(&db);
        let compressed = gzip_compress(&data);
        
        // Save as sensors_YYYYMMDD.json.gz
        save_compressed_db(&compressed);
        
        // Delete original .db file
        fs::remove_file(&db)?;
    }
}
```

**Format**:
```json
{
  "date_range": "2024-03-01 to 2024-03-07",
  "readings": [...],
  "calibrations": [...]
}
```

**Benefits**:
- SQLite file: ~1 MB/week
- Compressed JSON: ~50 KB/week
- **Reduction**: 95% space savings
- Still queryable (decompress on demand)

### 3.3 Garbage Collection Service
```rust
fn cleanup_old_data() {
    // Run daily
    compress_old_models();
    compress_old_databases();
    delete_redundant_models();
}

fn delete_redundant_models() {
    // Keep:
    // - Latest model (always)
    // - One per day for last 7 days
    // - One per week for last 30 days
    // - One per month forever
    
    let models = list_all_models();
    let to_keep = calculate_retention_policy(models);
    let to_delete = models.difference(&to_keep);
    
    for model in to_delete {
        fs::remove_file(model)?;
    }
}
```

## Phase 4: Bottom Location Selector

### UI Design
```
╔═══════════════════════════════════╗
║  [Live] [Model] [History] [📍]    ║  ← ViewSwitcher with 4 tabs
╠═══════════════════════════════════╣
║                                   ║
║         Content Area              ║
║                                   ║
╠═══════════════════════════════════╣
║  📍 Montreal  ▼                   ║  ← Location selector
╚═══════════════════════════════════╝
```

### Implementation
```rust
// Add to bottom of window
let location_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
location_bar.set_halign(gtk::Align::Center);
location_bar.set_margin_all(8);

let location_label = gtk::Label::new(Some("📍"));
let location_dropdown = gtk::DropDown::new(...);

location_bar.append(&location_label);
location_bar.append(&location_dropdown);

main_box.append(&location_bar);
```

## Phase 5: Interactive Swipe (Advanced)

### Current State
- ViewStack handles tab switching
- No visual feedback during swipe

### Goal
- Pan gesture shows preview of next page
- Elastic snap to page
- Smooth 60 FPS animation

### Implementation
```rust
let pan = gtk::GesturePan::new(gtk::Orientation::Horizontal);
pan.connect_pan(move |_, direction, offset| {
    // Calculate progress 0.0 to 1.0
    let progress = (offset / window_width).clamp(0.0, 1.0);
    
    // Update transition progress
    stack.set_transition_running(true);
    stack.set_transition_position(progress);
});

pan.connect_end(move |_| {
    // Snap to nearest page
    if progress > 0.5 {
        stack.set_visible_child_name(next_page);
    }
    stack.set_transition_running(false);
});
```

**Challenge**: GTK4 ViewStack doesn't expose `set_transition_position()`
**Solution**: Use custom Stack widget or AdwCarousel

## Implementation Order

### Week 1: Locations
- [x] Data structures
- [x] Helper functions  
- [ ] Location tab UI
- [ ] Add location dialog
- [ ] Location selector

### Week 2: Database Optimization
- [ ] Model compression
- [ ] Database compression
- [ ] Garbage collection cron
- [ ] Migration tool for existing data

### Week 3: Polish
- [ ] Dynamic padding on Page 1
- [ ] Interactive page swipe (if feasible)
- [ ] Better loading indicators
- [ ] Performance testing

## Testing Checklist

### Location Management
- [ ] Add city location (Montreal)
- [ ] Add network location (pin to current WiFi)
- [ ] Switch between locations
- [ ] Location persists across restarts
- [ ] Delete location

### Compression
- [ ] Models compress after 24h
- [ ] Databases compress after 7 days
- [ ] Can still query old data
- [ ] Space usage < 50 MB after 1 month

### UX
- [ ] Swipe between history weeks
- [ ] Swipe between main pages (if implemented)
- [ ] Spinner shows during loads
- [ ] No empty white space
- [ ] Responsive button presses

## Performance Targets

### Disk Usage
- Current: ~200 MB/month
- Target: <20 MB/month
- **90% reduction**

### Model Files
- Current: ~43,000 files/month
- Target: <50 files/month  
- **99.9% reduction**

### Response Time
- Page switch: <16ms (60 FPS)
- Location switch: <100ms
- Compression: Background (doesn't block UI)

## Migration Plan

### For Existing Users
```rust
fn migrate_v1_to_v2() {
    println!("Migrating data to v0.2.0...");
    
    // Step 1: Compress old models
    let models = glob("model_*.json");
    compress_by_month(models);
    
    // Step 2: Compress old databases
    let dbs = glob("sensors_*.db");
    compress_by_week(dbs);
    
    // Step 3: Create default location
    let locations = vec![
        Location::new("Unknown".to_string(), LocationType::Unknown)
    ];
    save_locations(&locations)?;
    
    println!("Migration complete!");
}
```

Run automatically on first launch of v0.2.0.

## File Structure After Implementation

```
~/.local/share/ambient-learner/
├── locations.json              # Location list
├── model_latest.json           # Current model (uncompressed)
├── models_2024-03.json.gz      # March 2024 models (compressed)
├── models_2024-04.json.gz      # April 2024 models
├── sensors_week_2024-03-01.json.gz   # Week archives
├── sensors_week_2024-03-08.json.gz
└── sensors_current.db          # This week's database
```

**Before**: 50,000+ files
**After**: ~50 files
**Improvement**: 99.9% fewer files

## Next Steps

1. Build and test current changes
2. Implement Location tab UI
3. Add location selector bar
4. Implement compression algorithms
5. Test migration on dummy data
6. Polish UX interactions
