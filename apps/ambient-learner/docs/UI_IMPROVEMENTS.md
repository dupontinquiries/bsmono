# UI/UX Improvements - Consistency & Polish

## Issues Identified

### 1. **Model Prediction Instability with Limited Data** ✅ FIXED
**Problem:** Neural network gives wonky predictions with <20 calibrations
**Solution:** 
- Use linear model for calibrations 0-19 (stable)
- Switch to neural network at calibration 20+
- Train both models simultaneously
- Clear indicator showing which model is active

**Code:**
```rust
fn predict(&mut self, readings: &[f32]) -> f32 {
    if self.calibration_count >= 20 && neural_initialized {
        self.predict_neural(readings)  // Accurate with enough data
    } else {
        self.predict_linear(readings)  // Stable with limited data
    }
}
```

**Status Display:**
```
Page 1: Active sensors: 18 | Calibrations: 15 (Linear)
        Active sensors: 18 | Calibrations: 25 (Neural)
```

---

### 2. **Database Count Discrepancy** ✅ FIXED
**Problem:** Page 5 showed database count (53), Page 1 showed model count (15)
**Root Cause:** Database has all historical calibrations, model only uses recent ones
**Solution:** Both pages now show model calibration count for consistency

**Before:**
```
Page 1: Calibrations: 15
Page 5: 53 calibrations  ← Confusing!
```

**After:**
```
Page 1: Calibrations: 15 (Linear)
Page 5: 15 calibrations (Linear)  ← Consistent!
```

---

### 3. **UI Consistency Issues** 🔨 TODO

**Problem Areas:**
- Inconsistent colors across pages
- Varying font sizes without clear hierarchy
- Different background colors
- No unified design language

**Current State:**
- ✅ Page 1 (Live): Good - nice colors, clear hierarchy
- ✅ Page 3 (History): Good - clean design
- ⚠️ Page 2 (Model): Functional but bland
- ⚠️ Page 4 (Locations): Basic list
- ⚠️ Page 5 (Sync): Just added, needs polish

---

## Proposed UI Improvements

### Color Palette (Consistent Across All Pages)

**Primary Colors:**
```css
--glass-bg: rgba(255, 255, 255, 0.05)
--glass-border: rgba(255, 255, 255, 0.15)
--accent-blue: #74c0fc
--accent-green: #7ED321
--accent-red: #E24A4A
--accent-yellow: #ffd43b
```

**Temperature Gradient (Page 1):**
```rust
fn temp_to_color(temp: f32) -> &'static str {
    match temp {
        t if t < 0.0 => "#4fc3f7",    // Ice blue
        t if t < 10.0 => "#74c0fc",   // Cool blue
        t if t < 20.0 => "#7ED321",   // Perfect green
        t if t < 25.0 => "#ffd43b",   // Warm yellow
        t if t < 30.0 => "#ffa500",   // Hot orange
        _ => "#E24A4A",               // Very hot red
    }
}
```

**Apply to All Pages:**
- Page 1: Keep current (perfect!)
- Page 2: Add temperature-based glow to SVG
- Page 3: Keep current (clean!)
- Page 4: Add accent colors for location types
- Page 5: Add status colors (green=ready, blue=importing, etc.)

---

### Typography Hierarchy

**Title Sizes (Consistent):**
```css
.title-1  { font-size: 28pt; font-weight: bold; }   /* Page headers */
.title-2  { font-size: 20pt; font-weight: medium; } /* Section headers */
.title-3  { font-size: 16pt; font-weight: medium; } /* Subsections */
.title-4  { font-size: 14pt; font-weight: normal; } /* Details */
.display  { font-size: 80pt; font-weight: bold; }   /* Main temp */
```

**Current Usage:**
- Page 1: ✅ Good - display (80pt) → title-2 (20pt) → title-4 (14pt)
- Page 2: ⚠️ Needs work - no clear hierarchy
- Page 3: ✅ Good - title-3 for chart labels
- Page 4: ⚠️ Basic - just row titles
- Page 5: ⚠️ Just added - needs refinement

---

### Background Consistency

**Glass Panel Styles:**
```css
.glass-panel {
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 16px;
}

.glass-panel-subtle {
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
}
```

**Apply to:**
- Page 1: ✅ Already has glass-panel
- Page 2: Add glass-panel to SVG container
- Page 3: ✅ Already has glass effects
- Page 4: Add glass-panel-subtle to list items
- Page 5: ✅ Already added glass-panel

---

## Specific Page Improvements

### Page 2 (Model Visualization)

**Current Issues:**
- SVG too technical
- No temperature context
- Bland presentation

**Improvements:**
```rust
// Add temperature-based glow to SVG
let glow_color = temp_to_color(current_temp);
svg.push_str(&format!(
    "<filter id='glow'><feGaussianBlur stdDeviation='4'/>
     <feColorMatrix type='matrix' values='...'/>
     </filter>"
));

// Add current temperature overlay
svg.push_str(&format!(
    "<text x='50%' y='20' text-anchor='middle' 
          fill='{}' font-size='24' font-weight='bold'>
     {:.1}°C
    </text>",
    glow_color, current_temp
));

// Add model type indicator
svg.push_str(&format!(
    "<text x='50%' y='45' text-anchor='middle' 
          fill='#888' font-size='14'>
     {} Model | {} calibrations
    </text>",
    if calibrations >= 20 { "Neural" } else { "Linear" },
    calibrations
));
```

---

### Page 4 (Locations)

**Current Issues:**
- Plain list
- No visual distinction
- No context

**Improvements:**
```rust
// Add icons and colors based on location type
let (icon, color) = match location.location_type {
    LocationType::City(_) => ("📍", "#74c0fc"),      // Blue for cities
    LocationType::NetworkId(_) => ("📶", "#7ED321"), // Green for networks
    LocationType::Unknown => ("❓", "#888"),         // Gray for unknown
};

row.set_icon_name(Some(icon));
row.add_css_class(&format!("location-{}", color));

// Add calibration count per location
let cal_count = db.get_calibration_count_for_location(&location.id)?;
row.set_subtitle(&format!("{} | {} calibrations", subtitle, cal_count));
```

---

### Page 5 (Database Sync)

**Current Issues:**
- Just a basic list
- No visual feedback
- Static information

**Improvements:**
```rust
// Make row updateable
let current_db_row_subtitle = gtk::Label::new(None);
current_db_row.add_suffix(&current_db_row_subtitle);

// Update in UI loop
if calibrations != last_calibrations {
    let model_type = if calibrations >= 20 { "Neural" } else { "Linear" };
    let model_color = if calibrations >= 20 { "#7ED321" } else { "#74c0fc" };
    
    current_db_row_subtitle.set_markup(&format!(
        "<span foreground='{}'>📊 {} calibrations ({})</span>",
        model_color, calibrations, model_type
    ));
}

// Add import progress indicator
let import_progress = gtk::ProgressBar::new();
import_progress.set_visible(false);
import_box.append(&import_progress);

// Show during import
import_btn.connect_clicked(move |_| {
    import_progress.set_visible(true);
    import_progress.set_fraction(0.5);
    import_progress.set_text(Some("Importing..."));
    
    // ... import logic ...
    
    import_progress.set_fraction(1.0);
    import_progress.set_text(Some("Complete!"));
    
    glib::timeout_add_local_once(Duration::from_secs(2), move || {
        import_progress.set_visible(false);
    });
});
```

---

## Database Robustness Improvements

### Current Issue: Loss on Error
**Problem:** If database has any error, we create a new one and lose data
**Root Cause:** No merge/repair logic

### Solution: Database Recovery & Merge

```rust
// At startup, instead of:
let db = if let Some(db_path) = get_latest_database() {
    Database::open(&db_path)  // Fails → lose database
} else {
    Database::new(&get_timestamp())
};

// Do this:
let db = if let Some(db_path) = get_latest_database() {
    match Database::open(&db_path) {
        Ok(db) => {
            println!("✅ Opened database: {}", db_path);
            db
        }
        Err(e) => {
            eprintln!("⚠️  Failed to open database: {}", e);
            
            // Try to recover/repair
            if let Ok(repaired_db) = repair_database(&db_path) {
                println!("✅ Database repaired successfully");
                repaired_db
            } else {
                // Corruption - try to salvage what we can
                eprintln!("⚠️  Database corrupted, salvaging data...");
                
                // Create new database
                let new_db = Database::new(&get_timestamp());
                
                // Try to extract calibrations from corrupted database
                if let Ok(calibrations) = salvage_calibrations(&db_path) {
                    println!("✅ Salvaged {} calibrations", calibrations.len());
                    for cal in calibrations {
                        let _ = new_db.insert_calibration(cal);
                    }
                }
                
                // Rename corrupted database
                let backup_path = format!("{}.corrupted", db_path);
                let _ = std::fs::rename(&db_path, &backup_path);
                println!("📦 Corrupted database backed up to: {}", backup_path);
                
                new_db
            }
        }
    }
} else {
    Database::new(&get_timestamp())
};
```

**Recovery Functions:**
```rust
fn repair_database(path: &str) -> Result<Database, Error> {
    // Try VACUUM to rebuild database file
    let conn = Connection::open(path)?;
    conn.execute("VACUUM", [])?;
    conn.execute("REINDEX", [])?;
    
    // Verify integrity
    let integrity: String = conn.query_row(
        "PRAGMA integrity_check",
        [],
        |row| row.get(0)
    )?;
    
    if integrity == "ok" {
        Ok(Database { /* ... */ })
    } else {
        Err(Error::Corruption)
    }
}

fn salvage_calibrations(path: &str) -> Result<Vec<Calibration>, Error> {
    let conn = Connection::open(path)?;
    
    // Try to read calibrations even from partially corrupted database
    let mut stmt = conn.prepare(
        "SELECT * FROM calibration_data WHERE timestamp > 0"
    )?;
    
    let calibrations: Vec<Calibration> = stmt
        .query_map([], |row| {
            Ok(Calibration {
                timestamp: row.get(0)?,
                temperature: row.get(1)?,
                // ... other fields
            })
        })?
        .filter_map(Result::ok)  // Skip corrupted rows
        .collect();
    
    Ok(calibrations)
}
```

---

## Quick Wins (Immediate Improvements)

### 1. **Model Type Indicator** ✅ IMPLEMENTED
```
Status: Active sensors: 18 | Calibrations: 15 (Linear)
                                                 ^^^^^^
```

### 2. **Switch at 20 Calibrations** ✅ IMPLEMENTED
```rust
if self.calibration_count == 20 {
    println!("🧠 Switching to neural network (20+ calibrations reached)");
}
```

### 3. **Consistent Calibration Counts** ✅ IMPLEMENTED
Both Page 1 and Page 5 now show model count

---

## Testing Checklist

### Model Switching
- [ ] With 0-19 calibrations → Shows "(Linear)"
- [ ] At calibration 20 → Message "🧠 Switching to neural network"
- [ ] With 20+ calibrations → Shows "(Neural)"
- [ ] Predictions stable with <20 calibrations
- [ ] Predictions improve with 20+ calibrations

### UI Consistency
- [ ] All pages have consistent glass effects
- [ ] Colors match across pages
- [ ] Font sizes follow hierarchy
- [ ] No jarring transitions

### Database Robustness
- [ ] Corrupted database → Salvage what's possible
- [ ] Error opening database → Try repair first
- [ ] Complete failure → Backup corrupted file
- [ ] No data loss on recoverable errors

---

## Build & Test

```bash
cargo build --release
./target/release/ambient-learner
```

**Test with fresh start:**
1. Delete old databases: `rm weather_data/*.db*`
2. Launch app
3. Add 5 calibrations → Should show "Linear"
4. Continue to 20 calibrations → Should switch to "Neural"
5. Check predictions are stable throughout

**Test database recovery:**
1. Corrupt database: `dd if=/dev/zero of=weather_data/*.db bs=1024 count=1`
2. Launch app
3. Should salvage data or create new database cleanly

---

## Summary

✅ **Fixed:**
- Model instability with limited data
- Database count discrepancy
- Model type visibility

🔨 **TODO (Next Session):**
- Full UI consistency pass
- Page 2 improvements (SVG with context)
- Page 4 improvements (colored icons)
- Page 5 improvements (live updates)
- Database recovery/salvage logic

**Status:** Core issues fixed, app should be much more usable now!
