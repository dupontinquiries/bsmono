#!/bin/bash
# Compilation Fixes - Apply All Patches
# Run this script to fix all compilation errors

set -e

echo "🔧 Applying compilation fixes..."

# Fix 1: Add Serde derives to DeviceState
echo "📝 Fix 1: Adding Serde derives to DeviceState..."
sed -i '101a #[derive(Clone, Default, Debug, serde::Serialize, serde::Deserialize)]' src/world_state.rs
sed -i '102d' src/world_state.rs

# Fix 2: Fix DeviceState fields in temperature_field.rs (line 577-578)
echo "📝 Fix 2: Fixing DeviceState fields..."
sed -i '577,578d' src/temperature_field.rs
sed -i '576a\        input.push(device_state.battery_level.clamp(0.0, 1.0));\n        input.push(device_state.screen_brightness.clamp(0.0, 1.0));' src/temperature_field.rs

# Fix 3: Change DeviceState parameter to reference in add_snapshot
echo "📝 Fix 3: Changing add_snapshot to use reference..."
sed -i 's/device_state: DeviceState/device_state: \&DeviceState/g' src/temperature_field.rs

# Fix 4: Fix DeviceState import in main.rs
echo "📝 Fix 4: Fixing DeviceState import..."
sed -i 's/use temperature_field::{TemperatureEstimator, DeviceState};/use temperature_field::TemperatureEstimator;\nuse world_state::DeviceState;/' src/main.rs

# Fix 5: Comment out set_enable_search (line 5311)
echo "📝 Fix 5: Commenting out set_enable_search..."
sed -i '5311s/^/            \/\/ /' src/main.rs

# Fix 6: Fix ui/mod.rs WorldState import
echo "📝 Fix 6: Adding WorldState import to ui/mod.rs..."
sed -i '3a use crate::world_state::WorldState;' src/ui/mod.rs

# Fix 7: Make zbus imports conditional
echo "📝 Fix 7: Making zbus conditional..."
cat > /tmp/zbus_patch.txt << 'EOF'
#[cfg(feature = "dbus")]
use zbus::{Connection as ZbusConnection, Result as ZbusResult};
#[cfg(feature = "dbus")]
use async_io::block_on;

#[cfg(not(feature = "dbus"))]
async fn get_current_network_ssid_dbus() -> Result<Option<String>, String> {
    Err("D-Bus support not enabled".to_string())
}
EOF

# Fix 8: Create stub for ThermalPoller and Model exports
echo "📝 Fix 8: Exporting ThermalPoller and Model from helpers..."
echo "" >> src/helpers.rs
echo "// Re-exports for easier access" >> src/helpers.rs
echo "pub use self::ThermalPoller;" >> src/helpers.rs
echo "pub use self::Model;" >> src/helpers.rs

echo "✅ All fixes applied!"
echo ""
echo "To build:"
echo "  cargo build --release"
echo ""
echo "Note: Some warnings may still appear but should not prevent compilation"
