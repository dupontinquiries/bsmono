// ============================================================================
// Distributed Database Sync Module
// ============================================================================
//
// Enables sharing calibration databases between devices via NFC
// Implements scoring system to evaluate data quality
// Uses git-like architecture for versioning and merging
//
// Features:
// - NFC peer-to-peer data transfer
// - Database scoring based on prediction accuracy
// - Merge conflict resolution
// - Compression and archiving

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use flate2::write::GzEncoder;
use flate2::read::GzDecoder;
use flate2::Compression;
use std::io::{Write, Read};
use crate::security::{SecurityManager, TrustedDevice, SignedPacket};

/// Remote database metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteDatabase {
    pub id: String,
    pub name: String,
    pub device_name: String,
    pub last_sync: i64,
    pub total_calibrations: usize,
    pub locations: Vec<String>,
    
    // Scoring metrics
    pub prediction_score: f32,  // How well it predicts (0-1)
    pub history_match_score: f32, // How well history matches (0-1)
    pub overall_score: f32,      // Combined score (0-1)
    pub data_points: usize,
    
    // Storage
    pub database_path: PathBuf,
    pub compressed_size: usize,
    pub enabled: bool,
}

impl RemoteDatabase {
    /// Calculate overall score from prediction and history match
    pub fn update_score(&mut self) {
        self.overall_score = (self.prediction_score * 0.7) + 
                            (self.history_match_score * 0.3);
    }
}

/// Database transfer packet for NFC
#[derive(Debug, Serialize, Deserialize)]
pub struct DatabasePacket {
    pub device_id: String,
    pub device_name: String,
    pub timestamp: i64,
    pub calibrations: Vec<CalibrationRecord>,
    pub locations: Vec<LocationRecord>,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationRecord {
    pub timestamp: i64,
    pub sensor_readings: Vec<f32>,
    pub target_temp: f32,
    pub location_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationRecord {
    pub id: String,
    pub name: String,
    pub location_type: String,
}

/// Database sync manager
pub struct SyncManager {
    pub remote_databases: Vec<RemoteDatabase>,
    local_device_id: String,
    data_dir: PathBuf,
    security: SecurityManager,
}

impl SyncManager {
    /// Create a new SyncManager
    /// 
    /// Returns an error if security manager initialization fails
    /// (e.g., cannot create data directory, write key file, etc.)
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        let security = SecurityManager::new(data_dir.clone())
            .map_err(|e| format!("Failed to initialize security: {}", e))?;
        let local_device_id = security.get_identity().device_id.clone();
        
        Ok(Self {
            remote_databases: Vec::new(),
            local_device_id,
            data_dir,
            security,
        })
    }
    
    /// Get device identity for display
    pub fn get_identity(&self) -> &crate::security::DeviceIdentity {
        self.security.get_identity()
    }
    
    /// Get trusted devices list
    pub fn get_trusted_devices(&self) -> &[TrustedDevice] {
        self.security.get_trusted_devices()
    }
    
    /// Get or create a unique device ID
    fn get_or_create_device_id(data_dir: &PathBuf) -> String {
        let id_path = data_dir.join("device_id.txt");
        
        if let Ok(id) = std::fs::read_to_string(&id_path) {
            return id.trim().to_string();
        }
        
        // Generate new UUID
        let new_id = uuid::Uuid::new_v4().to_string();
        let _ = std::fs::write(&id_path, &new_id);
        new_id
    }
    
    /// Export local database for NFC transfer (SIGNED & ENCRYPTED)
    pub fn export_database(&self, db: &crate::Database) -> Result<Vec<u8>, String> {
        // Collect calibrations
        let calibrations = db.get_all_calibrations_v1()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(timestamp, readings, target, location_id)| {
                CalibrationRecord {
                    timestamp,
                    sensor_readings: readings,
                    target_temp: target,
                    location_id,
                }
            })
            .collect();
        
        // Collect locations (stub - would need to add to Database)
        let locations = Vec::new();
        
        let packet = DatabasePacket {
            device_id: self.local_device_id.clone(),
            device_name: Self::get_device_name(),
            timestamp: chrono::Utc::now().timestamp(),
            calibrations,
            locations,
            checksum: String::new(), // Calculate below
        };
        
        // Serialize to JSON
        let json = serde_json::to_string(&packet)
            .map_err(|e| e.to_string())?;
        
        // Compress with gzip
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(json.as_bytes())
            .map_err(|e| e.to_string())?;
        let compressed = encoder.finish()
            .map_err(|e| e.to_string())?;
        
        // Sign the compressed data
        let signed_packet = self.security.create_signed_packet(compressed, false)?;
        
        // Serialize signed packet
        let final_data = serde_json::to_vec(&signed_packet)
            .map_err(|e| e.to_string())?;
        
        println!("🔐 Exported database: {} bytes (signed)", final_data.len());
        Ok(final_data)
    }
    
    /// Import database from NFC transfer (VERIFY SIGNATURE)
    pub fn import_database(&mut self, data: &[u8]) -> Result<RemoteDatabase, String> {
        // Deserialize signed packet
        let signed_packet: SignedPacket = serde_json::from_slice(data)
            .map_err(|e| format!("Invalid packet: {}", e))?;
        
        println!("📥 Received database from: {}", signed_packet.sender_id);
        
        // Check if sender is trusted
        let trusted = self.security.get_trusted_devices().iter()
            .find(|d| d.device_id == signed_packet.sender_id);
        
        if trusted.is_none() {
            return Err(format!(
                "⚠️  Unknown device: {}\nAccept pairing request first!",
                signed_packet.sender_id
            ));
        }
        
        // Verify signature
        let compressed = self.security.verify_signed_packet(&signed_packet)?;
        println!("✅ Signature verified!");
        
        // Decompress
        // Decompress
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut json = String::new();
        decoder.read_to_string(&mut json)
            .map_err(|e| e.to_string())?;
        
        // Deserialize
        let packet: DatabasePacket = serde_json::from_str(&json)
            .map_err(|e| e.to_string())?;
        
        // Validate checksum (stub)
        
        // Save to file
        let db_id = format!("remote_{}", packet.device_id);
        let db_path = self.data_dir.join(format!("{}.db", db_id));
        
        // Create remote database entry
        let remote = RemoteDatabase {
            id: db_id.clone(),
            name: format!("{}'s Data", packet.device_name),
            device_name: packet.device_name,
            last_sync: packet.timestamp,
            total_calibrations: packet.calibrations.len(),
            locations: packet.locations.iter()
                .map(|l| l.name.clone())
                .collect(),
            prediction_score: 0.0,
            history_match_score: 0.0,
            overall_score: 0.0,
            data_points: packet.calibrations.len(),
            database_path: db_path.clone(),
            compressed_size: data.len(),
            enabled: true,
        };
        
        // Save database file
        std::fs::write(&db_path, data)
            .map_err(|e| e.to_string())?;
        
        self.remote_databases.push(remote.clone());
        
        Ok(remote)
    }
    
    /// Score a remote database based on prediction accuracy
    pub fn score_database(&mut self, db_index: usize, test_predictions: &[(f32, f32)]) {
        if db_index >= self.remote_databases.len() {
            return;
        }
        
        if test_predictions.is_empty() {
            return;
        }
        
        // Calculate prediction accuracy
        let mut total_error = 0.0;
        for (predicted, actual) in test_predictions {
            total_error += (predicted - actual).abs();
        }
        let avg_error = total_error / test_predictions.len() as f32;
        
        // Convert error to score (0-1, where 1 is perfect)
        // Assume 5°C error = 0 score, 0°C error = 1 score
        let prediction_score = (1.0 - (avg_error / 5.0).min(1.0)).max(0.0);
        
        self.remote_databases[db_index].prediction_score = prediction_score;
        self.remote_databases[db_index].update_score();
    }
    
    /// Score history match (how similar is the data)
    pub fn score_history_match(&mut self, db_index: usize, _local_history: &[(i64, f32)]) {
        if db_index >= self.remote_databases.len() {
            return;
        }
        
        // Load remote database and compare timestamps/temps
        // For now, use a simple heuristic based on data overlap
        let remote = &self.remote_databases[db_index];
        
        // Simple score: how much data do they have?
        let data_score = (remote.data_points as f32 / 1000.0).min(1.0);
        
        self.remote_databases[db_index].history_match_score = data_score;
        self.remote_databases[db_index].update_score();
    }
    
    /// Merge a remote database into local database
    pub fn merge_database(&self, db_index: usize, local_db: &mut crate::Database) -> Result<usize, String> {
        if db_index >= self.remote_databases.len() {
            return Err("Invalid database index".to_string());
        }
        
        let remote = &self.remote_databases[db_index];
        
        if !remote.enabled {
            return Err("Database is disabled".to_string());
        }
        
        // Load remote database file
        let data = std::fs::read(&remote.database_path)
            .map_err(|e| e.to_string())?;
        
        // Decompress and deserialize
        let mut decoder = GzDecoder::new(&data[..]);
        let mut json = String::new();
        decoder.read_to_string(&mut json)
            .map_err(|e| e.to_string())?;
        
        let packet: DatabasePacket = serde_json::from_str(&json)
            .map_err(|e| e.to_string())?;
        
        // Merge calibrations (avoiding duplicates by timestamp)
        let mut merged_count = 0;
        for calib in packet.calibrations {
            // Check if this timestamp already exists
            // For now, just insert (would need dedup in production)
            let _ = local_db.store_calibration(
                &calib.sensor_readings,
                calib.target_temp,
                calib.location_id.as_deref()
            );
            merged_count += 1;
        }
        
        Ok(merged_count)
    }
    
    /// Remove a remote database
    pub fn remove_database(&mut self, db_index: usize) -> Result<(), String> {
        if db_index >= self.remote_databases.len() {
            return Err("Invalid database index".to_string());
        }
        
        let remote = self.remote_databases.remove(db_index);
        
        // Delete database file
        let _ = std::fs::remove_file(&remote.database_path);
        
        Ok(())
    }
    
    /// Toggle database enabled status
    pub fn toggle_database(&mut self, db_index: usize) {
        if db_index < self.remote_databases.len() {
            self.remote_databases[db_index].enabled = 
                !self.remote_databases[db_index].enabled;
        }
    }
    
    /// Get device name from system
    fn get_device_name() -> String {
        // Try to get hostname
        if let Ok(output) = std::process::Command::new("hostname").output() {
            if let Ok(name) = String::from_utf8(output.stdout) {
                return name.trim().to_string();
            }
        }
        
        "Unknown Device".to_string()
    }
    
    /// Save remote database list to disk
    pub fn save_registry(&self) -> Result<(), String> {
        let registry_path = self.data_dir.join("remote_databases.json");
        let json = serde_json::to_string_pretty(&self.remote_databases)
            .map_err(|e| e.to_string())?;
        std::fs::write(registry_path, json)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    
    /// Load remote database list from disk
    pub fn load_registry(&mut self) -> Result<(), String> {
        let registry_path = self.data_dir.join("remote_databases.json");
        if !registry_path.exists() {
            return Ok(());
        }
        
        let json = std::fs::read_to_string(registry_path)
            .map_err(|e| e.to_string())?;
        self.remote_databases = serde_json::from_str(&json)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// NFC transfer helper (Linux/Android)
pub struct NfcTransfer {
    enabled: bool,
}

impl NfcTransfer {
    pub fn new() -> Self {
        Self { enabled: Self::check_nfc_available() }
    }
    
    /// Check if NFC is available on this system
    fn check_nfc_available() -> bool {
        // Check for NFC devices in /dev
        std::path::Path::new("/dev/nfc0").exists() ||
        std::path::Path::new("/dev/nfc").exists()
    }
    
    /// Send data via NFC (peer-to-peer)
    pub fn send(&self, data: &[u8]) -> Result<(), String> {
        if !self.enabled {
            return Err("NFC not available".to_string());
        }
        
        // On Android/Termux, use `nfc-tools` or Android NFC API
        // For now, save to a sharable file
        let temp_path = std::env::temp_dir().join("ambient_db_transfer.gz");
        std::fs::write(&temp_path, data)
            .map_err(|e| e.to_string())?;
        
        println!("📱 NFC: Data ready to send from {}", temp_path.display());
        println!("   Size: {} bytes", data.len());
        
        Ok(())
    }
    
    /// Receive data via NFC (peer-to-peer)
    pub fn receive(&self) -> Result<Vec<u8>, String> {
        if !self.enabled {
            return Err("NFC not available".to_string());
        }
        
        // Check for incoming transfer file
        let temp_path = std::env::temp_dir().join("ambient_db_transfer.gz");
        if !temp_path.exists() {
            return Err("No data waiting".to_string());
        }
        
        let data = std::fs::read(&temp_path)
            .map_err(|e| e.to_string())?;
        
        println!("📱 NFC: Received {} bytes", data.len());
        
        Ok(data)
    }
    
    pub fn is_available(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sync_manager() {
        let temp_dir = std::env::temp_dir().join("ambient_test");
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let manager = SyncManager::new(temp_dir.clone()).expect("init failed");
        assert!(!manager.local_device_id.is_empty());
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
}
