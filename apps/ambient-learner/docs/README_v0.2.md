# Local Weather App v0.2.0 - Modern GNOME Edition

A sophisticated ambient temperature prediction app for mobile Linux using **libadwaita** for a polished GNOME/Phosh look.

## 🎉 What's New in v0.2.0

### Modern UI with libadwaita
- **Native GNOME design** with Adwaita styling
- **Adaptive layouts** that work on desktop and mobile
- **Smooth animations** and modern widgets
- **Header bar** with proper GNOME integration
- **View switcher** at bottom for easy thumb navigation

### Model Persistence
- **Auto-save every 60 seconds** to timestamped files
- **Snapshot naming**: `model_20250103_143022.json`
- **Auto-load latest** model on startup
- **Never lose calibrations** even if app crashes

### Database Snapshots
- **Timestamped databases**: `sensors_20250103_143022.db`
- **One DB per session** with ISO timestamps
- **History page aggregates** all databases
- **Never lose historical data**

### Sensor Locking & Safety
- **Exclusive sensor locks** prevent conflicts
- **Multiple instances detected** and blocked
- **Missing data points** properly handled
- **Visual indicator** shows sensor lock status
- **Error dialogs** if calibration attempted without lock

### Enhanced History Page
- **Week-by-week navigation** (← Previous / Next Week →)
- **Shows 7 days** at a time (default: this week)
- **Drag through time** week by week into the past
- **Aggregates all databases** automatically
- **Dark theme** with normalized graphs

## 📱 Three Pages

### Page 1: Live Temperature
- Large color-coded temperature display
- Confidence percentage
- Active sensor count
- Calibration count
- **Sensor lock status indicator**

### Page 2: Neural Network
- Animated network visualization
- Staggered layout for 20+ sensors
- Color-coded nodes (green/yellow/gray)
- Connection weights visible
- **Calibrate button** (checks sensor lock first)

### Page 3: Temperature History  
- 7-day bar chart (normalized)
- **Week navigation buttons**
- Summary statistics (avg, min, max)
- **Aggregates all historical databases**
- Dark theme, no white-on-dark

## 🗄️ File Structure

```
weather_data/
├── model_20250103_143022.json      # Neural network snapshot
├── model_20250103_150000.json      # Auto-saved every minute
├── sensors_20250103_143022.db      # Session database
├── sensors_20250104_080000.db      # Next session
└── ...
```

### Model Files (JSON)
```json
{
  "stats": [...],
  "bias": 2.34,
  "last_prediction": 15.2,
  "calibration_count": 39,
  "created_at": "20250103_143022"
}
```

### Database Schema
```sql
-- readings: hourly sensor snapshots
timestamp INTEGER PRIMARY KEY
data TEXT  -- JSON with sensors[], cpu_load, etc.

-- calibrations: user calibration points
timestamp INTEGER PRIMARY KEY
sensors TEXT  -- averaged sensor values
target REAL   -- user-provided temp
```

## 🔒 Sensor Locking

The app now properly handles sensor access:

1. **On startup**: Attempts to lock all thermal sensors
2. **During polling**: Validates lock on each read
3. **If lock fails**: Shows "⚠ Sensors Unavailable" in red
4. **During calibration**: Blocks if sensors not locked
5. **Error dialog**: "Cannot calibrate: thermal sensors are locked by another process"

This prevents:
- Multiple instances fighting over sensors
- Bouncing temperature readings
- Bad calibration data from incomplete reads

## 🎯 Usage

### First Launch
1. App auto-loads latest model (or creates new one)
2. App creates new database with current timestamp
3. Passive polling starts (1/sec)
4. Calibrate when ready

### Calibration
1. Check "● Sensors Locked" is green
2. Go to Network page
3. Press "Calibrate Model"
4. Enter known temperature
5. App collects 50 burst samples + 15s passive history
6. Model updates and **auto-saves immediately**

### History Navigation
1. Go to History page
2. Shows this week by default
3. Press "← Previous Week" to go back
4. Press "Next Week →" to return
5. Automatically aggregates all database files

## 🔧 Building

### postmarketOS (aarch64)
```bash
sudo apk add rust cargo gtk4.0-dev libadwaita-dev pkgconfig gcc musl-dev
cargo build --release
```

### Ubuntu/Debian
```bash
sudo apt install libgtk-4-dev libadwaita-1-dev
cargo build --release
```

### Run
```bash
./target/release/ambient-learner
```

## 📊 How It Works

### Snapshot System
- **Model**: Saved every 60 seconds with timestamp
- **Database**: New file per app session
- **On startup**: Loads latest model + creates new DB
- **On shutdown**: Model already saved, DB persists
- **History**: Queries all `sensors_*.db` files

### Sensor Locking
```rust
fn try_lock(&mut self) -> bool {
    // Attempt to read all sensors
    // If any fail, lock is lost
    // Return false, skip this data point
}

fn poll(&mut self) -> Option<Vec<f32>> {
    // Returns None if lock not held
    // UI shows red warning
    // Calibration blocked
}
```

### Week Navigation
```rust
week_offset = 0   // This week
week_offset = 7   // 1 week ago
week_offset = 14  // 2 weeks ago

get_daily_averages(offset, 7)  // Get 7 days starting from offset
```

## 🎨 Modern UI Features

### libadwaita Widgets
- `adw::ApplicationWindow` - Adaptive window
- `adw::HeaderBar` - GNOME-style header
- `adw::ViewStack` - Page container
- `adw::ViewSwitcherBar` - Bottom navigation
- `adw::MessageDialog` - Native dialogs
- `adw::EntryRow` - Styled input fields

### CSS Classes
- `.title-1` - Large headings
- `.title-4` - Subheadings
- `.dim-label` - Secondary text
- `.caption` - Small text
- `.suggested-action` - Primary button
- `.pill` - Rounded button

### Adaptive Layout
- Works on PinePhone (720x1440)
- Works on desktop (any size)
- Touch-friendly spacing
- Thumb-accessible switcher

## 🔬 Technical Details

### Model Persistence
- JSON format for human readability
- Includes all sensor stats + weights
- Timestamp in filename for sorting
- Latest file auto-detected via regex

### Database Strategy
- **Why snapshots?** Prevents corruption from crashes
- **Why timestamps?** Easy chronological sorting
- **Why multiple DBs?** Each session independent
- **How to clean?** Delete old `sensors_*.db` files

### Sensor Safety
- Exclusive file handle per sensor
- Validation on every read
- Fail-fast if any sensor unavailable
- Visual feedback to user

## 📈 Performance

### Memory Usage
- ~20-30 MB (model + buffers)
- Minimal growth over time
- Old models/DBs can be deleted

### Disk Usage
- Model: ~5-10 KB each (saved every 60s)
- Database: Grows ~1 KB/hour (hourly snapshots)
- After 1 week: ~100 models (~1 MB) + ~168 KB data

### Battery Impact
- Passive polling: Negligible (<1%)
- Active burst: Brief spike during calibration
- UI updates: 5 FPS (efficient)

## 🚀 Future Ideas

- [ ] Model pruning (delete old snapshots)
- [ ] Database compaction
- [ ] Export calibration data
- [ ] Import from other devices
- [ ] Cloud sync (optional)
- [ ] Background service mode
- [ ] Lock file instead of sensor locking
- [ ] CPU load sensor integration
- [ ] Ambient light sensor (ALS)

## 🐛 Troubleshooting

### "Sensors Unavailable"
- Another instance is running
- Close other instances
- Or: `killall ambient-learner`

### No history data
- Wait 1 hour for first hourly save
- Or: Force save by restarting app

### Model not loading
- Check `weather_data/model_*.json` exists
- Delete corrupt files if needed
- App will create new model

### Week navigation stuck
- Not enough historical data
- Need at least 7 days for previous week

## 📜 License

Your code, your rules!

## 🙏 Credits

Built with:
- **GTK4** - Cross-platform UI
- **libadwaita** - GNOME design language
- **rusqlite** - SQLite bindings
- **serde** - JSON serialization
- **chrono** - Timestamp handling

Designed for mobile Linux (postmarketOS, Mobian, etc.)
