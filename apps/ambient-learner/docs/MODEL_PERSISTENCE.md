# Model Persistence Troubleshooting Guide

## How Model Persistence Works

The application automatically saves your trained model in two scenarios:

1. **Every 60 seconds** - Automatic background save
2. **After calibration** - Immediate save when you calibrate the model

Models are saved to: `~/.local/share/ambient-learner/model_YYYYMMDD_HHMMSS.json`

## Checking if Model Persistence is Working

### Step 1: Check Terminal Output

When you run the application, you should see messages like:

```
=== Ambient Learner v0.2.0 ===
Data directory: /home/username/.local/share/ambient-learner
Found existing model: /home/username/.local/share/ambient-learner/model_20260304_143022.json
✓ Model loaded successfully! Calibrations: 3, Active sensors: 8
Found existing database: /home/username/.local/share/ambient-learner/sensors_20260304_143022.db
```

If you see **"No existing model found. Creating new model."** on subsequent runs, something is wrong.

### Step 2: Verify Files Exist

Check your data directory:

```bash
ls -lh ~/.local/share/ambient-learner/

# You should see files like:
# model_20260304_143022.json
# model_20260304_144022.json  (saved 10 minutes later)
# sensors_20260304_143022.db
```

### Step 3: Check Model Content

View a model file to verify it has data:

```bash
cat ~/.local/share/ambient-learner/model_*.json | head -20

# Should show JSON with your calibration data:
# {
#   "stats": [
#     {
#       "weight": 0.234,
#       "variance": 0.045,
#       ...
```

## Common Issues and Fixes

### Issue 1: "No existing model found" Every Time

**Cause**: Directory permissions or model file corruption

**Fix**:
```bash
# Check directory exists and is writable
ls -ld ~/.local/share/ambient-learner/
# Should show: drwxr-xr-x (user has write permission)

# If it doesn't exist, create it:
mkdir -p ~/.local/share/ambient-learner/
chmod 755 ~/.local/share/ambient-learner/

# Check if model files exist but are corrupt:
ls -lh ~/.local/share/ambient-learner/model_*.json

# Try to read one:
cat ~/.local/share/ambient-learner/model_*.json | python3 -m json.tool
# If this fails, the JSON is corrupt - delete it and recalibrate
```

### Issue 2: Model Saves But Doesn't Load

**Cause**: Model file exists but contains data from different sensors

**Symptoms**: 
- Terminal shows "Found existing model" but then "Failed to load model"
- Error messages about sensor mismatch

**Fix**:
```bash
# Check your thermal sensors
ls /sys/class/thermal/thermal_zone*/temp

# If you've changed hardware or the number of sensors changed,
# delete old models and recalibrate:
rm ~/.local/share/ambient-learner/model_*.json

# Restart the app and recalibrate
```

### Issue 3: Permission Denied Errors

**Cause**: Directory not writable

**Fix**:
```bash
# Fix directory permissions
chmod 755 ~/.local/share/ambient-learner/
chmod 644 ~/.local/share/ambient-learner/*.json
chmod 644 ~/.local/share/ambient-learner/*.db
```

### Issue 4: Model Saves Every Minute Creating Many Files

**Expected Behavior**: This is normal! The app saves every 60 seconds to prevent data loss.

**To Clean Up Old Models**:
```bash
# Keep only the 10 most recent models
cd ~/.local/share/ambient-learner/
ls -t model_*.json | tail -n +11 | xargs rm -f

# Or keep models from last 7 days only
find ~/.local/share/ambient-learner/ -name "model_*.json" -mtime +7 -delete
```

## Verifying Model Is Actually Loading

1. **Calibrate the model** (note the confidence % after calibration)
2. **Close the app completely**
3. **Check that a model file exists**:
   ```bash
   ls -lh ~/.local/share/ambient-learner/model_*.json
   ```
4. **Restart the app and check terminal output**:
   - Should say "Found existing model"
   - Should say "✓ Model loaded successfully! Calibrations: X"
5. **Check the confidence %** - it should match what you saw before closing

## Debug Mode

To see detailed logging, run the app from terminal:

```bash
./ambient-learner 2>&1 | tee ambient-learner.log
```

Look for these messages:
- "Model saved: ..." (should appear every 60 seconds)
- "Model saved after calibration: ..." (after you calibrate)
- "✓ Model loaded successfully!" (on startup)

## Manual Model Inspection

You can manually inspect a model file:

```bash
# Pretty-print the model
python3 -c "
import json
with open('/home/username/.local/share/ambient-learner/model_20260304_143022.json') as f:
    data = json.load(f)
    print('Calibrations:', data['calibration_count'])
    print('Number of sensors:', len(data['stats']))
    print('Active sensors:', sum(1 for s in data['stats'] if s['dropout_score'] > 0.1))
"
```

## Complete Reset

If all else fails, start fresh:

```bash
# Backup old data
mv ~/.local/share/ambient-learner ~/.local/share/ambient-learner.backup

# Create fresh directory
mkdir -p ~/.local/share/ambient-learner

# Restart app and recalibrate
./ambient-learner
```

## What Gets Persisted

✅ **Saved**:
- Sensor weights
- Calibration count
- Sensor dropout scores
- Sensor variances
- Last prediction value

❌ **Not Saved** (recreated on startup):
- Real-time sensor readings
- Last 5 minutes of history buffer
- UI state (which tab is open)

## Expected File Sizes

- **Model files**: 1-5 KB (depends on number of sensors)
- **Database files**: Grows over time
  - Fresh: ~16 KB
  - After 1 day: ~50-100 KB
  - After 1 week: ~500 KB - 1 MB

If your model file is 0 bytes or much larger than expected, it's corrupt.

## Contact / Issues

If model persistence still doesn't work after trying all these steps:

1. Run with logging: `./ambient-learner 2>&1 | tee debug.log`
2. Calibrate once
3. Close and restart
4. Share the debug.log output

The log will show exactly where the save/load process is failing.
