# Location-Based Temperature Forecasting - Design Document

## Overview

Add location awareness to enable historical temperature forecasting based on where the device is, using multiple identification methods with privacy-preserving cryptographic hashing.

## Feature: Location Identification Methods

### Method 1: WiFi Network Fingerprinting
**Concept**: Identify location by visible WiFi SSIDs (like indoor GPS)

**Implementation**:
```rust
struct WiFiFingerprint {
    ssid_hashes: Vec<[u8; 32]>,  // SHA-256 of SSIDs
    signal_strengths: Vec<i8>,
    timestamp: u64,
}

impl WiFiFingerprint {
    fn from_scan() -> Self {
        // Scan visible networks
        let networks = scan_wifi_networks();
        
        // Hash SSIDs with salt (non-reversible)
        let ssid_hashes = networks.iter()
            .map(|ssid| {
                let mut hasher = Sha256::new();
                hasher.update(SALT);  // App-specific salt
                hasher.update(ssid.as_bytes());
                hasher.finalize().into()
            })
            .collect();
            
        Self {
            ssid_hashes,
            signal_strengths: networks.iter().map(|n| n.rssi).collect(),
            timestamp: now(),
        }
    }
    
    fn similarity(&self, other: &WiFiFingerprint) -> f32 {
        // Jaccard similarity of SSID sets
        let set1: HashSet<_> = self.ssid_hashes.iter().collect();
        let set2: HashSet<_> = other.ssid_hashes.iter().collect();
        
        let intersection = set1.intersection(&set2).count();
        let union = set1.union(&set2).count();
        
        intersection as f32 / union as f32
    }
}
```

**Privacy**: 
- SSIDs are hashed with SHA-256 (irreversible)
- Can be shared without revealing actual network names
- Similarity matching works on hashed values

### Method 2: Audio Fingerprinting (Shazam-like)
**Concept**: Create acoustic fingerprint of a location using ambient sound

**Implementation**:
```rust
struct AudioFingerprint {
    spectral_peaks: Vec<SpectralPeak>,
    duration_seconds: u32,
    hash: [u8; 32],
}

struct SpectralPeak {
    frequency: u16,      // Hz
    time_offset: u16,    // ms from start
    magnitude: u8,       // Relative strength
}

impl AudioFingerprint {
    fn from_recording(duration_secs: u32) -> Self {
        // Record ambient audio (30-60 seconds recommended)
        let audio = record_audio(duration_secs);
        
        // Compute STFT (Short-Time Fourier Transform)
        let stft = compute_stft(&audio);
        
        // Extract spectral peaks (like Shazam)
        let peaks = extract_peaks(&stft);
        
        // Create constellation map
        let constellation = create_constellation(&peaks);
        
        // Hash the constellation (non-reversible)
        let hash = hash_constellation(&constellation);
        
        Self {
            spectral_peaks: constellation,
            duration_seconds: duration_secs,
            hash,
        }
    }
    
    fn similarity(&self, other: &AudioFingerprint) -> f32 {
        // Compare constellation patterns
        let matches = count_matching_peaks(&self.spectral_peaks, &other.spectral_peaks);
        
        matches as f32 / self.spectral_peaks.len().max(other.spectral_peaks.len()) as f32
    }
}
```

**Privacy**:
- Audio is never stored, only spectral fingerprint
- Fingerprint is hashed (irreversible to original sound)
- Cannot be used to reconstruct conversation
- Only identifies acoustic environment pattern

**Advantages**:
- Works indoors where GPS fails
- Unique to specific rooms/locations
- Passive (always-on ambient sound)
- Privacy-preserving

### Method 3: City Name Fallback
**Simple user-entered location**:
```rust
struct CityLocation {
    city_name: String,
    country_code: String,
    coordinates: Option<(f64, f64)>,  // Geocoded from name
}
```

## Feature: Location-Based Forecast

### Data Structure

```rust
struct Location {
    id: Uuid,
    name: String,  // User-friendly name
    city_fallback: Option<CityLocation>,
    wifi_fingerprint: Option<WiFiFingerprint>,
    audio_fingerprint: Option<AudioFingerprint>,
    created_at: DateTime<Utc>,
}

struct LocationReading {
    location_id: Uuid,
    temperature: f32,
    timestamp: DateTime<Utc>,
    confidence: f32,  // How confident we are about location match
}

struct Forecast {
    location_id: Uuid,
    predictions: Vec<HourlyPrediction>,
}

struct HourlyPrediction {
    hour: u8,  // 0-23
    predicted_temp: f32,
    confidence: f32,
    based_on_samples: usize,
}
```

### Location Detection Algorithm

```rust
fn detect_current_location(
    wifi_scan: Option<WiFiFingerprint>,
    audio_sample: Option<AudioFingerprint>,
    known_locations: &[Location]
) -> Option<(Uuid, f32)> {
    let mut best_match: Option<(Uuid, f32)> = None;
    
    for location in known_locations {
        let mut similarity = 0.0;
        let mut weight_sum = 0.0;
        
        // WiFi similarity (weight: 0.6)
        if let (Some(wifi_scan), Some(wifi_fp)) = (&wifi_scan, &location.wifi_fingerprint) {
            similarity += wifi_scan.similarity(wifi_fp) * 0.6;
            weight_sum += 0.6;
        }
        
        // Audio similarity (weight: 0.4)
        if let (Some(audio), Some(audio_fp)) = (&audio_sample, &location.audio_fingerprint) {
            similarity += audio.similarity(audio_fp) * 0.4;
            weight_sum += 0.4;
        }
        
        if weight_sum > 0.0 {
            let normalized_similarity = similarity / weight_sum;
            
            // Require >70% similarity to consider it a match
            if normalized_similarity > 0.7 {
                if best_match.is_none() || normalized_similarity > best_match.unwrap().1 {
                    best_match = Some((location.id, normalized_similarity));
                }
            }
        }
    }
    
    best_match
}
```

### Forecast Generation

```rust
fn generate_forecast(location_id: Uuid, db: &Database) -> Forecast {
    // Get all historical readings for this location
    let readings = db.get_location_readings(location_id);
    
    // Group by hour of day
    let mut hourly_data: HashMap<u8, Vec<f32>> = HashMap::new();
    for reading in readings {
        let hour = reading.timestamp.hour() as u8;
        hourly_data.entry(hour).or_default().push(reading.temperature);
    }
    
    // Calculate predictions
    let predictions = (0..24).map(|hour| {
        if let Some(temps) = hourly_data.get(&hour) {
            let avg = temps.iter().sum::<f32>() / temps.len() as f32;
            let variance = temps.iter()
                .map(|t| (t - avg).powi(2))
                .sum::<f32>() / temps.len() as f32;
            
            HourlyPrediction {
                hour,
                predicted_temp: avg,
                confidence: 1.0 / (1.0 + variance),
                based_on_samples: temps.len(),
            }
        } else {
            HourlyPrediction {
                hour,
                predicted_temp: 20.0,  // Default
                confidence: 0.0,
                based_on_samples: 0,
            }
        }
    }).collect();
    
    Forecast {
        location_id,
        predictions,
    }
}
```

## UI Design

### New Page: Locations

```
╔═══════════════════════════════════╗
║  📍 Locations                     ║
╠═══════════════════════════════════╣
║                                   ║
║  🏠 Home                          ║
║     WiFi: 4 networks  🟢         ║
║     Last seen: 2 min ago          ║
║     Avg temp: 22.5°C              ║
║     [View Forecast]               ║
║                                   ║
║  🏢 Office                        ║
║     WiFi: 8 networks  🟢         ║
║     Audio: Recorded   🟢         ║
║     Last seen: 8 hours ago        ║
║     Avg temp: 23.1°C              ║
║     [View Forecast]               ║
║                                   ║
║  ➕ [Add New Location]            ║
║                                   ║
╚═══════════════════════════════════╝
```

### Add Location Dialog

```
╔═══════════════════════════════════╗
║  Add New Location                 ║
╠═══════════════════════════════════╣
║                                   ║
║  Name: [________________]         ║
║                                   ║
║  City (optional):                 ║
║  [________________]               ║
║                                   ║
║  ☐ Capture WiFi fingerprint       ║
║  ☐ Record audio fingerprint       ║
║      Duration: [30s ▼]            ║
║                                   ║
║  [Cancel]  [Create Location]      ║
║                                   ║
╚═══════════════════════════════════╝
```

### Location Forecast View

```
╔═══════════════════════════════════╗
║  🏠 Home - Temperature Forecast   ║
╠═══════════════════════════════════╣
║                                   ║
║   📊 24-Hour Prediction           ║
║   ╭───────────────────────────╮  ║
║   │        ╱╲                 │  ║
║   │       ╱  ╲    ╱╲          │  ║
║   │  ────╱────╲──╱──╲────     │  ║
║   │  6am 12pm  6pm 12am       │  ║
║   ╰───────────────────────────╯  ║
║                                   ║
║   Current: 22.3°C                 ║
║   Predicted (6pm): 24.1°C ±1.2°  ║
║                                   ║
║   Based on 47 historical readings ║
║   Confidence: 89%                 ║
║                                   ║
║   [View History] [Edit Location]  ║
║                                   ║
╚═══════════════════════════════════╝
```

## Sharing Locations

### Export Format

```json
{
  "location": {
    "name": "Home",
    "wifi_fingerprint": {
      "ssid_hashes": [
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "..."
      ],
      "signal_pattern": [/* normalized strengths */]
    },
    "audio_fingerprint": {
      "hash": "7f83b1657ff1fc53b92dc18148a1d65dfc2d4b1fa3d677284addd200126d9069",
      "spectral_peaks": [/* peak data */]
    },
    "city": {
      "name": "Montreal",
      "country": "CA"
    }
  },
  "signature": "...",  // HMAC to verify authenticity
  "version": "1.0"
}
```

### Import/Export

```rust
impl Location {
    fn export(&self) -> Result<String, Error> {
        // Serialize to JSON
        let json = serde_json::to_string_pretty(&self)?;
        
        // Sign with HMAC to prevent tampering
        let signature = sign_hmac(&json, &get_app_key());
        
        let export = LocationExport {
            location: json,
            signature,
            version: "1.0".to_string(),
        };
        
        Ok(serde_json::to_string(&export)?)
    }
    
    fn import(data: &str) -> Result<Self, Error> {
        let export: LocationExport = serde_json::from_str(data)?;
        
        // Verify signature
        if !verify_hmac(&export.location, &export.signature, &get_app_key()) {
            return Err(Error::InvalidSignature);
        }
        
        Ok(serde_json::from_str(&export.location)?)
    }
}
```

## Privacy & Security

### Cryptographic Hashing

**Why non-reversible?**
- WiFi SSIDs might reveal personal info ("John's iPhone")
- Audio could contain speech fragments
- Want shareable without privacy leakage

**Implementation**:
```rust
const APP_SALT: &[u8] = b"ambient-learner-v1-location-salt";

fn hash_ssid(ssid: &str) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(APP_SALT);
    hasher.update(ssid.as_bytes());
    hasher.finalize().into()
}

fn hash_audio_constellation(peaks: &[SpectralPeak]) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(APP_SALT);
    for peak in peaks {
        hasher.update(&peak.frequency.to_le_bytes());
        hasher.update(&peak.time_offset.to_le_bytes());
        hasher.update(&[peak.magnitude]);
    }
    hasher.finalize().into()
}
```

### Data Retention

- Audio recording: Immediately deleted after fingerprint extracted
- WiFi scans: Only hashes stored, raw SSIDs never saved
- Location matches: Logged with confidence scores
- User can delete any location and all associated data

## Dependencies

```toml
[dependencies]
# Existing
gtk4 = "0.7"
libadwaita = "0.5"
# ... others ...

# New for location features
sha2 = "0.10"  # SHA-256 hashing
hmac = "0.12"  # HMAC signatures
uuid = { version = "1.0", features = ["v4", "serde"] }

# Audio processing
cpal = "0.15"  # Cross-platform audio I/O
rustfft = "6.0"  # FFT for spectral analysis

# WiFi scanning
# Linux: NetworkManager DBus
# macOS: CoreWLAN framework
# Windows: Native WiFi API
```

## Implementation Phases

### Phase 1: Basic Location Management
- [ ] Location CRUD (Create, Read, Update, Delete)
- [ ] City name fallback
- [ ] Manual temperature logging per location
- [ ] Simple forecast from historical data

### Phase 2: WiFi Fingerprinting
- [ ] WiFi network scanning
- [ ] SSID hashing
- [ ] Similarity matching
- [ ] Auto-detection on WiFi change

### Phase 3: Audio Fingerprinting
- [ ] Audio recording UI
- [ ] STFT computation
- [ ] Spectral peak extraction
- [ ] Constellation map creation
- [ ] Background audio monitoring (optional)

### Phase 4: Forecasting
- [ ] Hourly prediction algorithm
- [ ] Confidence scoring
- [ ] Forecast visualization
- [ ] Comparison with actual temps

### Phase 5: Sharing
- [ ] Export location fingerprints
- [ ] Import shared locations
- [ ] QR code generation
- [ ] Community location database (optional)

## Technical Challenges

### WiFi Scanning Permissions
- **Linux**: Requires NetworkManager, works out of box
- **macOS**: Requires location permissions
- **Windows**: Requires admin for some APIs

**Solution**: Graceful degradation - if WiFi unavailable, use audio + city

### Audio Processing Performance
- FFT is CPU-intensive
- Need to run in background thread
- Consider using GPU for STFT if available

**Solution**: 
- Process in 1-2 second chunks
- Use rustfft (well-optimized)
- Option to disable audio fingerprinting

### False Positives
- Two locations might have similar WiFi
- Audio might change (construction, events)

**Solution**:
- Require high similarity threshold (>80%)
- Allow manual location override
- Let user mark "similar but different" locations

## Future Enhancements

- **Weather API integration**: Compare predictions with actual weather
- **ML-based forecasting**: Use LSTM for better predictions
- **Multi-location tracking**: Detect when you move between known locations
- **Geofencing**: Auto-switch when entering/leaving area
- **Community insights**: "Other people at this location report..."
- **Anomaly detection**: Alert if temp is unusual for location/time

## Conclusion

This location-based forecasting system enables:
- Privacy-preserving location identification
- Personalized temperature predictions
- Shareable location fingerprints
- Fallback to simple city-based tracking

The multi-method approach (WiFi + Audio + City) ensures it works in various scenarios while maintaining user privacy through cryptographic hashing.
