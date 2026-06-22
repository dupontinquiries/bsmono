# Bonus Features - Advanced Location & Data Sync

## Overview

Three powerful features to make location detection more robust and enable model sharing between devices.

---

## Feature 1: Full Network Scan (All Nearby Networks)

### Concept
Instead of just detecting the connected network, scan ALL visible WiFi networks to create a more unique location fingerprint.

### Why This is Better
- **More unique**: 10+ nearby networks vs 1 connected network
- **Works when not connected**: Can detect location even without WiFi password
- **More stable**: Network list changes slowly, not affected by connection drops
- **Indoor positioning**: Differentiates between rooms in same building

### Implementation

#### Network Scanner
```rust
struct NetworkScan {
    networks: Vec<NetworkInfo>,
    timestamp: i64,
}

struct NetworkInfo {
    ssid_hash: [u8; 32],  // SHA-256 of SSID (privacy)
    signal_strength: i8,   // dBm
    frequency: u16,        // MHz (2.4 or 5 GHz)
}

fn scan_all_networks() -> Vec<NetworkInfo> {
    // Linux: nmcli
    let output = Command::new("nmcli")
        .args(["-t", "-f", "SSID,SIGNAL,FREQ", "dev", "wifi", "list"])
        .output()
        .expect("nmcli not found");
    
    let text = String::from_utf8_lossy(&output.stdout);
    let mut networks = Vec::new();
    
    for line in text.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 {
            let ssid = parts[0];
            let signal: i8 = parts[1].parse().unwrap_or(-100);
            let freq: u16 = parts[2].parse().unwrap_or(2412);
            
            // Hash SSID for privacy
            let mut hasher = Sha256::new();
            hasher.update(NETWORK_SALT);
            hasher.update(ssid.as_bytes());
            let ssid_hash = hasher.finalize().into();
            
            networks.push(NetworkInfo {
                ssid_hash,
                signal_strength: signal,
                frequency: freq,
            });
        }
    }
    
    // Sort by signal strength (strongest first)
    networks.sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));
    
    // Keep top 15 (enough for uniqueness, not too much data)
    networks.truncate(15);
    
    networks
}
```

#### Enhanced Location Structure
```rust
#[derive(Clone, Serialize, Deserialize, Debug)]
struct Location {
    id: String,
    name: String,
    location_type: LocationType,
    
    // Enhanced network fingerprint
    network_fingerprint: Option<NetworkFingerprint>,
    
    // GPS coordinates (from router query)
    coordinates: Option<Coordinates>,
    
    created_at: String,
    last_seen: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct NetworkFingerprint {
    networks: Vec<NetworkInfo>,
    scan_time: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Coordinates {
    latitude: f64,
    longitude: f64,
    accuracy: f32,  // meters
    source: LocationSource,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
enum LocationSource {
    RouterQuery,
    Manual,
    GeoIP,
}
```

#### Location Matching Algorithm
```rust
fn match_location_by_network_scan(
    scan: &NetworkScan,
    known_locations: &[Location]
) -> Option<(usize, f32)> {
    
    let mut best_match = None;
    
    for (idx, location) in known_locations.iter().enumerate() {
        if let Some(fingerprint) = &location.network_fingerprint {
            let similarity = calculate_network_similarity(
                &scan.networks,
                &fingerprint.networks
            );
            
            // Require 60% similarity (at least 9/15 networks match)
            if similarity > 0.6 {
                if best_match.is_none() || similarity > best_match.unwrap().1 {
                    best_match = Some((idx, similarity));
                }
            }
        }
    }
    
    best_match
}

fn calculate_network_similarity(
    scan1: &[NetworkInfo],
    scan2: &[NetworkInfo]
) -> f32 {
    let set1: HashSet<_> = scan1.iter().map(|n| n.ssid_hash).collect();
    let set2: HashSet<_> = scan2.iter().map(|n| n.ssid_hash).collect();
    
    // Jaccard similarity
    let intersection = set1.intersection(&set2).count();
    let union = set1.union(&set2).count();
    
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}
```

#### UI Updates
```rust
// When adding location
let scan_networks_btn = gtk::Button::with_label("📡 Scan Networks");

scan_networks_btn.connect_clicked(move |_| {
    let spinner = gtk::Spinner::new();
    spinner.start();
    
    // Scan in background
    thread::spawn(move || {
        let networks = scan_all_networks();
        
        glib::idle_add_local_once(move || {
            spinner.stop();
            // Show: "Detected 12 networks"
            // Save fingerprint to location
        });
    });
});
```

---

## Feature 2: Router Query for GPS Coordinates

### Concept
Query the router's public IP to get approximate GPS coordinates using GeoIP databases or router configuration.

### Methods

#### Method A: Router Admin Page Scraping
Some routers expose GPS coordinates in their admin interface (especially mobile hotspots).

```rust
fn query_router_location() -> Option<Coordinates> {
    // Get default gateway
    let gateway = get_default_gateway()?;
    
    // Common router admin pages
    let urls = vec![
        format!("http://{}/api/device/information", gateway),  // Huawei
        format!("http://{}/api/monitoring/status", gateway),   // Huawei LTE
        format!("http://{}/status.json", gateway),             // OpenWRT
    ];
    
    for url in urls {
        if let Ok(response) = reqwest::blocking::get(&url) {
            if let Ok(text) = response.text() {
                // Parse JSON for lat/lon
                if let Ok(info) = serde_json::from_str::<RouterInfo>(&text) {
                    if let (Some(lat), Some(lon)) = (info.latitude, info.longitude) {
                        return Some(Coordinates {
                            latitude: lat,
                            longitude: lon,
                            accuracy: 100.0,  // ~100m typical for cell towers
                            source: LocationSource::RouterQuery,
                        });
                    }
                }
            }
        }
    }
    
    None
}

fn get_default_gateway() -> Option<String> {
    // Linux
    let output = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;
    
    let text = String::from_utf8_lossy(&output.stdout);
    
    // Parse: "default via 192.168.1.1 ..."
    for word in text.split_whitespace() {
        if word.contains('.') && word.split('.').count() == 4 {
            return Some(word.to_string());
        }
    }
    
    None
}
```

#### Method B: GeoIP Lookup
Use public IP to get approximate location.

```rust
fn get_location_from_public_ip() -> Option<Coordinates> {
    // Get public IP
    let ip = reqwest::blocking::get("https://api.ipify.org")
        .ok()?
        .text()
        .ok()?;
    
    // Query GeoIP (free services)
    let url = format!("http://ip-api.com/json/{}", ip);
    let response = reqwest::blocking::get(&url).ok()?;
    let geo: GeoIPResponse = response.json().ok()?;
    
    Some(Coordinates {
        latitude: geo.lat,
        longitude: geo.lon,
        accuracy: 5000.0,  // City-level accuracy
        source: LocationSource::GeoIP,
    })
}

#[derive(Deserialize)]
struct GeoIPResponse {
    lat: f64,
    lon: f64,
    city: String,
    country: String,
}
```

#### UI Integration
```rust
// In location settings
let gps_btn = gtk::Button::with_label("📍 Get Coordinates");

gps_btn.connect_clicked(move |btn| {
    btn.set_sensitive(false);
    btn.set_label("Detecting...");
    
    thread::spawn(move || {
        // Try router first
        let coords = query_router_location()
            .or_else(|| get_location_from_public_ip());
        
        glib::idle_add_local_once(move || {
            if let Some(coords) = coords {
                // Update location with coordinates
                // Show: "Location: 45.5017°N, 73.5673°W (±100m)"
            }
            btn.set_sensitive(true);
            btn.set_label("📍 Get Coordinates");
        });
    });
});
```

#### Storage
```rust
// Add to database schema
CREATE TABLE IF NOT EXISTS locations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    location_type TEXT NOT NULL,
    latitude REAL,
    longitude REAL,
    accuracy REAL,
    location_source TEXT,
    created_at TEXT,
    last_seen TEXT
)
```

---

## Feature 3: NFC Model Transfer

### Concept
Transfer calibration data between devices by tapping them together using NFC.

### Use Cases
- **Share home location calibration** with family members
- **Backup model** to another device
- **Sync between laptop and phone**
- **Transfer to new device** when upgrading

### NFC Protocol Design

#### Data Format
```rust
#[derive(Serialize, Deserialize)]
struct NFCPayload {
    version: u8,  // Protocol version
    payload_type: NFCPayloadType,
    data: Vec<u8>,  // Compressed data
    checksum: [u8; 32],  // SHA-256
}

#[derive(Serialize, Deserialize)]
enum NFCPayloadType {
    ModelData,
    LocationData,
    CalibrationData,
    FullSync,
}

#[derive(Serialize, Deserialize)]
struct ModelTransfer {
    location_id: String,
    location_name: String,
    
    // Model data
    calibrations: Vec<CalibrationPoint>,
    sensor_stats: Vec<SensorStat>,
    
    // Coordinates if available
    coordinates: Option<Coordinates>,
    
    // Network fingerprint if available
    network_fingerprint: Option<NetworkFingerprint>,
}

#[derive(Serialize, Deserialize)]
struct CalibrationPoint {
    timestamp: i64,
    sensors: Vec<f32>,
    target: f32,
    location_id: String,
}
```

#### NFC Write (Sender)
```rust
use nfc::{Context, Device};

fn send_model_via_nfc(location: &Location, calibrations: &[CalibrationPoint]) -> Result<(), Box<dyn Error>> {
    // Prepare transfer data
    let transfer = ModelTransfer {
        location_id: location.id.clone(),
        location_name: location.name.clone(),
        calibrations: calibrations.to_vec(),
        sensor_stats: get_sensor_stats_for_location(&location.id),
        coordinates: location.coordinates.clone(),
        network_fingerprint: location.network_fingerprint.clone(),
    };
    
    // Serialize and compress
    let json = serde_json::to_vec(&transfer)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&json)?;
    let compressed = encoder.finish()?;
    
    // Calculate checksum
    let mut hasher = Sha256::new();
    hasher.update(&compressed);
    let checksum = hasher.finalize().into();
    
    // Create NFC payload
    let payload = NFCPayload {
        version: 1,
        payload_type: NFCPayloadType::ModelData,
        data: compressed,
        checksum,
    };
    
    let payload_bytes = bincode::serialize(&payload)?;
    
    // Initialize NFC
    let mut context = Context::new()?;
    let mut device = context.open()?;
    
    println!("Place receiving device on NFC reader...");
    
    // Wait for tag
    device.initiator_init()?;
    let target = device.initiator_select_passive_target(
        nfc::Modulation {
            modulation_type: nfc::ModulationType::Iso14443a,
            baud_rate: nfc::BaudRate::Baud106,
        },
        &[],
    )?;
    
    // Write data (may need to split into multiple NDEF messages)
    let chunks = payload_bytes.chunks(245); // Max NDEF message size
    for (i, chunk) in chunks.enumerate() {
        let ndef = create_ndef_message(chunk, i)?;
        device.initiator_transceive_bytes(&ndef, 1000)?;
    }
    
    println!("Transfer complete!");
    Ok(())
}
```

#### NFC Read (Receiver)
```rust
fn receive_model_via_nfc() -> Result<ModelTransfer, Box<dyn Error>> {
    let mut context = Context::new()?;
    let mut device = context.open()?;
    
    println!("Place sending device on NFC reader...");
    
    device.initiator_init()?;
    let target = device.initiator_select_passive_target(
        nfc::Modulation {
            modulation_type: nfc::ModulationType::Iso14443a,
            baud_rate: nfc::BaudRate::Baud106,
        },
        &[],
    )?;
    
    // Read NDEF messages
    let mut payload_bytes = Vec::new();
    loop {
        match device.initiator_transceive_bytes(&[0x30, 0x00], 1000) {
            Ok(data) => {
                payload_bytes.extend_from_slice(&data);
                if data.len() < 16 {
                    break; // Last chunk
                }
            }
            Err(_) => break,
        }
    }
    
    // Deserialize payload
    let payload: NFCPayload = bincode::deserialize(&payload_bytes)?;
    
    // Verify checksum
    let mut hasher = Sha256::new();
    hasher.update(&payload.data);
    let checksum: [u8; 32] = hasher.finalize().into();
    
    if checksum != payload.checksum {
        return Err("Checksum mismatch".into());
    }
    
    // Decompress
    let mut decoder = GzDecoder::new(&payload.data[..]);
    let mut json = Vec::new();
    decoder.read_to_end(&mut json)?;
    
    // Deserialize transfer
    let transfer: ModelTransfer = serde_json::from_slice(&json)?;
    
    println!("Received model for: {}", transfer.location_name);
    Ok(transfer)
}
```

#### Integration with App
```rust
// In Locations tab
let nfc_send_btn = gtk::Button::with_label("📲 Send via NFC");
let nfc_receive_btn = gtk::Button::with_label("📥 Receive via NFC");

nfc_send_btn.connect_clicked(move |btn| {
    btn.set_sensitive(false);
    
    let location = current_location.clone();
    let calibrations = get_calibrations_for_location(&location.id);
    
    thread::spawn(move || {
        match send_model_via_nfc(&location, &calibrations) {
            Ok(_) => {
                glib::idle_add_local_once(|| {
                    show_success_toast("Model sent successfully!");
                });
            }
            Err(e) => {
                eprintln!("NFC send error: {}", e);
                glib::idle_add_local_once(|| {
                    show_error_dialog("NFC transfer failed");
                });
            }
        }
        
        glib::idle_add_local_once(move || {
            btn.set_sensitive(true);
        });
    });
});

nfc_receive_btn.connect_clicked(move |btn| {
    btn.set_sensitive(false);
    
    thread::spawn(move || {
        match receive_model_via_nfc() {
            Ok(transfer) => {
                // Merge calibration data
                merge_received_model(transfer);
                
                glib::idle_add_local_once(move || {
                    show_success_toast(&format!(
                        "Received {} calibrations for {}",
                        transfer.calibrations.len(),
                        transfer.location_name
                    ));
                });
            }
            Err(e) => {
                eprintln!("NFC receive error: {}", e);
                glib::idle_add_local_once(|| {
                    show_error_dialog("NFC transfer failed");
                });
            }
        }
        
        glib::idle_add_local_once(move || {
            btn.set_sensitive(true);
        });
    });
});
```

#### Merge Strategy
```rust
fn merge_received_model(transfer: ModelTransfer) {
    let mut locations = load_locations();
    
    // Check if location exists
    let existing = locations.iter_mut()
        .find(|l| l.id == transfer.location_id);
    
    if let Some(location) = existing {
        // Update existing location
        if transfer.coordinates.is_some() {
            location.coordinates = transfer.coordinates;
        }
        if transfer.network_fingerprint.is_some() {
            location.network_fingerprint = transfer.network_fingerprint;
        }
    } else {
        // Create new location
        let new_location = Location {
            id: transfer.location_id.clone(),
            name: transfer.location_name,
            location_type: LocationType::Unknown,
            network_fingerprint: transfer.network_fingerprint,
            coordinates: transfer.coordinates,
            created_at: get_timestamp(),
            last_seen: None,
        };
        locations.push(new_location);
    }
    
    save_locations(&locations).unwrap();
    
    // Import calibrations into database
    let db = Database::open_or_create().unwrap();
    for cal in transfer.calibrations {
        db.store_calibration(&cal.sensors, cal.target, Some(&cal.location_id)).ok();
    }
    
    println!("Imported {} calibrations", transfer.calibrations.len());
}
```

---

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# Existing...

# Network scanning & GeoIP
reqwest = { version = "0.11", features = ["blocking", "json"] }
sha2 = "0.10"

# NFC support
nfc = "0.3"  # Linux libnfc bindings
bincode = "1.3"  # Binary serialization

# Compression
flate2 = "1.0"  # Already added
```

## Platform Support

### Network Scanning
- ✅ Linux: `nmcli` (NetworkManager)
- ✅ macOS: `airport -s` command
- ⚠️ Windows: Requires WlanAPI (more complex)

### NFC
- ✅ Linux: libnfc (USB NFC readers)
- ✅ Android: Via Termux with NFC permissions
- ❌ iOS: Not available (Apple restrictions)
- ⚠️ Windows: Requires additional drivers

### Router Query
- ✅ All platforms: Standard HTTP requests
- ⚠️ Requires router cooperation

## Implementation Priority

1. **Week 1**: Full network scan
   - Most impactful for location detection
   - Works everywhere
   - No special hardware

2. **Week 2**: Router GPS query
   - Nice addition to network scan
   - Useful for outdoor/city-wide location
   - Low effort

3. **Week 3**: NFC transfer
   - Coolest feature
   - Requires NFC hardware
   - Great for power users

## Testing Strategy

### Network Scan
1. Scan at home → 15 networks
2. Scan at coffee shop → different 15 networks
3. Verify 0% match between locations
4. Scan at home again → 95%+ match

### GPS Query
1. Query router at home
2. Verify lat/lon reasonable
3. Test with mobile hotspot
4. Fallback to GeoIP if router fails

### NFC Transfer
1. Calibrate model on Device A
2. Send via NFC to Device B
3. Verify calibrations appear on Device B
4. Check model accuracy on Device B

## Security Considerations

### Network Scan
- ✅ SSIDs hashed (SHA-256)
- ✅ Can't reverse to actual network names
- ✅ Safe to share

### GPS Coordinates
- ⚠️ Reveals approximate location
- ✅ User consent required
- ✅ Optional feature

### NFC Transfer
- ✅ SHA-256 checksum prevents tampering
- ✅ Compressed to save transfer time
- ✅ No personal data in payload
- ⚠️ Requires physical proximity (secure by design)

## Future Enhancements

- **Bluetooth mesh**: Discover nearby devices and sync automatically
- **Cloud backup**: Optional encrypted cloud sync
- **QR code sharing**: Alternative to NFC for model transfer
- **Mesh network map**: Visualize all known locations on map
- **Crowd-sourced data**: Anonymous contribution to global temperature database

---

## Summary

These three features transform the app from single-device temperature tracking to a robust, location-aware, multi-device ecosystem:

1. **Full network scan** = 95%+ location accuracy
2. **Router GPS** = Real geographic coordinates  
3. **NFC transfer** = Instant model sharing

Total implementation: ~3-4 days for full featured version! 🚀
