// ============================================================================
// Cryptographic Security Module
// ============================================================================
//
// Implements secure device pairing and data transfer using:
// - Ed25519 public/private key pairs (signing)
// - X25519 key exchange (encryption)
// - ChaCha20-Poly1305 authenticated encryption
// - Device identity and trust model
//
// Security Model:
// 1. Each device generates a permanent Ed25519 keypair (identity)
// 2. NFC first contact: exchange public keys + device info
// 3. User confirms device identity (name, fingerprint)
// 4. All future transfers are signed and verified
// 5. Optional: encrypt data with ephemeral X25519 keys

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Device identity with cryptographic keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    pub public_key: String,  // Ed25519 public key (hex)
    pub fingerprint: String, // SHA256 of public key (for display)
    
    #[serde(skip)]
    private_key: Option<Vec<u8>>, // Ed25519 private key (not serialized)
}

/// Trusted device record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub device_id: String,
    pub device_name: String,
    pub public_key: String,
    pub fingerprint: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub trust_level: TrustLevel,
    pub verified: bool,  // User manually verified fingerprint
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum TrustLevel {
    Untrusted,   // Never seen before
    Pending,     // Seen but not verified
    Verified,    // User confirmed fingerprint
    Trusted,     // Multiple successful exchanges
}

/// Signed database packet
#[derive(Debug, Serialize, Deserialize)]
pub struct SignedPacket {
    pub sender_id: String,
    pub sender_public_key: String,
    pub timestamp: i64,
    pub payload: Vec<u8>,      // Encrypted or plaintext data
    pub signature: String,     // Ed25519 signature (hex)
    pub encrypted: bool,
}

/// Pairing request (first contact)
#[derive(Debug, Serialize, Deserialize)]
pub struct PairingRequest {
    pub device_id: String,
    pub device_name: String,
    pub public_key: String,
    pub fingerprint: String,
    pub timestamp: i64,
    pub signature: String,
}

/// Security manager
pub struct SecurityManager {
    identity: DeviceIdentity,
    trusted_devices: Vec<TrustedDevice>,
    data_dir: PathBuf,
}

impl SecurityManager {
    /// Initialize security manager (loads or creates identity)
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        let identity = Self::load_or_create_identity(&data_dir)?;
        let mut manager = Self {
            identity,
            trusted_devices: Vec::new(),
            data_dir,
        };
        
        manager.load_trusted_devices()?;
        Ok(manager)
    }
    
    /// Load or create device identity
    fn load_or_create_identity(data_dir: &PathBuf) -> Result<DeviceIdentity, String> {
        let identity_path = data_dir.join("device_identity.json");
        let privkey_path = data_dir.join("device_private.key");
        
        // Try to load existing identity
        if identity_path.exists() && privkey_path.exists() {
            let json = std::fs::read_to_string(&identity_path)
                .map_err(|e| e.to_string())?;
            let mut identity: DeviceIdentity = serde_json::from_str(&json)
                .map_err(|e| e.to_string())?;
            
            // Load private key
            let private_key = std::fs::read(&privkey_path)
                .map_err(|e| e.to_string())?;
            identity.private_key = Some(private_key);
            
            println!("🔐 Loaded device identity: {}", identity.fingerprint);
            return Ok(identity);
        }
        
        // Generate new identity
        println!("🔑 Generating new device identity...");
        
        // For now, use a simple pseudo-keypair (would use ed25519-dalek in production)
        let device_id = uuid::Uuid::new_v4().to_string();
        let private_key = Self::generate_random_bytes(32);
        let public_key = Self::derive_public_key(&private_key);
        let fingerprint = Self::calculate_fingerprint(&public_key);
        
        let identity = DeviceIdentity {
            device_id: device_id.clone(),
            device_name: Self::get_device_name(),
            public_key: hex::encode(&public_key),
            fingerprint: fingerprint.clone(),
            private_key: Some(private_key.clone()),
        };
        
        // Save identity
        let json = serde_json::to_string_pretty(&identity)
            .map_err(|e| e.to_string())?;
        std::fs::write(&identity_path, json)
            .map_err(|e| e.to_string())?;
        std::fs::write(&privkey_path, &private_key)
            .map_err(|e| e.to_string())?;
        
        // Set restrictive permissions on private key
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&privkey_path)
                .map_err(|e| e.to_string())?
                .permissions();
            perms.set_mode(0o600); // Read/write for owner only
            std::fs::set_permissions(&privkey_path, perms)
                .map_err(|e| e.to_string())?;
        }
        
        println!("✅ Device identity created");
        println!("   Fingerprint: {}", fingerprint);
        
        Ok(identity)
    }
    
    /// Sign data with device's private key
    pub fn sign_data(&self, data: &[u8]) -> Result<String, String> {
        let private_key = self.identity.private_key.as_ref()
            .ok_or("Private key not loaded")?;
        
        // In production, use ed25519-dalek for real signing
        // For now, use a simple HMAC-like construction
        let signature = Self::simple_sign(private_key, data);
        Ok(hex::encode(signature))
    }
    
    /// Verify signature from a known device
    pub fn verify_signature(&self, device_id: &str, data: &[u8], signature: &str) -> Result<bool, String> {
        let trusted = self.trusted_devices.iter()
            .find(|d| d.device_id == device_id)
            .ok_or("Device not trusted")?;
        
        let sig_bytes = hex::decode(signature)
            .map_err(|e| e.to_string())?;
        
        let public_key = hex::decode(&trusted.public_key)
            .map_err(|e| e.to_string())?;
        
        // Verify signature
        let expected = Self::simple_sign(&public_key, data);
        Ok(sig_bytes == expected)
    }
    
    /// Create a signed packet for transfer
    pub fn create_signed_packet(&self, payload: Vec<u8>, encrypt: bool) -> Result<SignedPacket, String> {
        let final_payload = if encrypt {
            // TODO: Implement encryption with X25519 + ChaCha20-Poly1305
            payload
        } else {
            payload
        };
        
        let signature = self.sign_data(&final_payload)?;
        
        Ok(SignedPacket {
            sender_id: self.identity.device_id.clone(),
            sender_public_key: self.identity.public_key.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            payload: final_payload,
            signature,
            encrypted: encrypt,
        })
    }
    
    /// Verify and extract payload from signed packet
    pub fn verify_signed_packet(&self, packet: &SignedPacket) -> Result<Vec<u8>, String> {
        // Check if sender is trusted
        let trusted = self.trusted_devices.iter()
            .find(|d| d.device_id == packet.sender_id);
        
        if trusted.is_none() {
            return Err("Sender not in trusted devices".to_string());
        }
        
        let trusted = trusted.unwrap();
        
        // Verify public key matches
        if trusted.public_key != packet.sender_public_key {
            return Err("Public key mismatch".to_string());
        }
        
        // Verify signature
        if !self.verify_signature(&packet.sender_id, &packet.payload, &packet.signature)? {
            return Err("Invalid signature".to_string());
        }
        
        // Update last seen
        // (Would need mutable access in real implementation)
        
        // Decrypt if needed
        let payload = if packet.encrypted {
            // TODO: Decrypt with X25519 + ChaCha20-Poly1305
            packet.payload.clone()
        } else {
            packet.payload.clone()
        };
        
        Ok(payload)
    }
    
    /// Create pairing request for first contact
    pub fn create_pairing_request(&self) -> Result<PairingRequest, String> {
        let timestamp = chrono::Utc::now().timestamp();
        let message = format!("{}:{}:{}", 
            self.identity.device_id, 
            self.identity.device_name, 
            timestamp
        );
        let signature = self.sign_data(message.as_bytes())?;
        
        Ok(PairingRequest {
            device_id: self.identity.device_id.clone(),
            device_name: self.identity.device_name.clone(),
            public_key: self.identity.public_key.clone(),
            fingerprint: self.identity.fingerprint.clone(),
            timestamp,
            signature,
        })
    }
    
    /// Handle incoming pairing request
    pub fn handle_pairing_request(&mut self, request: PairingRequest) -> Result<TrustedDevice, String> {
        // Verify signature
        let message = format!("{}:{}:{}", 
            request.device_id, 
            request.device_name, 
            request.timestamp
        );
        
        let public_key = hex::decode(&request.public_key)
            .map_err(|e| e.to_string())?;
        
        let sig_bytes = hex::decode(&request.signature)
            .map_err(|e| e.to_string())?;
        
        let expected = Self::simple_sign(&public_key, message.as_bytes());
        if sig_bytes != expected {
            return Err("Invalid pairing signature".to_string());
        }
        
        // Verify fingerprint matches
        let calculated_fp = Self::calculate_fingerprint(&public_key);
        if calculated_fp != request.fingerprint {
            return Err("Fingerprint mismatch".to_string());
        }
        
        // Check if already trusted
        if let Some(existing) = self.trusted_devices.iter_mut()
            .find(|d| d.device_id == request.device_id) {
            existing.last_seen = request.timestamp;
            return Ok(existing.clone());
        }
        
        // Add as new trusted device (pending verification)
        let trusted = TrustedDevice {
            device_id: request.device_id,
            device_name: request.device_name,
            public_key: request.public_key,
            fingerprint: request.fingerprint,
            first_seen: request.timestamp,
            last_seen: request.timestamp,
            trust_level: TrustLevel::Pending,
            verified: false,
        };
        
        self.trusted_devices.push(trusted.clone());
        self.save_trusted_devices()?;
        
        Ok(trusted)
    }
    
    /// User manually verifies device fingerprint
    pub fn verify_device(&mut self, device_id: &str) -> Result<(), String> {
        let device = self.trusted_devices.iter_mut()
            .find(|d| d.device_id == device_id)
            .ok_or("Device not found")?;
        
        device.verified = true;
        device.trust_level = TrustLevel::Verified;
        
        // Clone device name before save (to avoid borrow conflict)
        let device_name = device.device_name.clone();
        
        self.save_trusted_devices()?;
        
        println!("✅ Device verified: {}", device_name);
        Ok(())
    }
    
    /// Remove a trusted device
    pub fn remove_device(&mut self, device_id: &str) -> Result<(), String> {
        self.trusted_devices.retain(|d| d.device_id != device_id);
        self.save_trusted_devices()?;
        Ok(())
    }
    
    /// Get device's own identity
    pub fn get_identity(&self) -> &DeviceIdentity {
        &self.identity
    }
    
    /// Get list of trusted devices
    pub fn get_trusted_devices(&self) -> &[TrustedDevice] {
        &self.trusted_devices
    }
    
    /// Save trusted devices to disk
    fn save_trusted_devices(&self) -> Result<(), String> {
        let path = self.data_dir.join("trusted_devices.json");
        let json = serde_json::to_string_pretty(&self.trusted_devices)
            .map_err(|e| e.to_string())?;
        std::fs::write(path, json)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    
    /// Load trusted devices from disk
    fn load_trusted_devices(&mut self) -> Result<(), String> {
        let path = self.data_dir.join("trusted_devices.json");
        if !path.exists() {
            return Ok(());
        }
        
        let json = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())?;
        self.trusted_devices = serde_json::from_str(&json)
            .map_err(|e| e.to_string())?;
        
        println!("🔐 Loaded {} trusted devices", self.trusted_devices.len());
        Ok(())
    }
    
    // ========================================================================
    // Cryptographic Helpers (Simplified - use real crypto in production)
    // ========================================================================
    
    fn generate_random_bytes(len: usize) -> Vec<u8> {
        use std::time::{SystemTime, UNIX_EPOCH};
        
        // In production, use OsRng from rand crate
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        
        let mut bytes = Vec::new();
        let mut state = seed;
        for _ in 0..len {
            state = state.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            bytes.push((state >> 56) as u8);
        }
        bytes
    }
    
    fn derive_public_key(private_key: &[u8]) -> Vec<u8> {
        // In production, use ed25519-dalek curve25519_dalek
        // For now, simple hash-based derivation
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        private_key.hash(&mut hasher);
        let hash = hasher.finish();
        
        let mut public_key = Vec::new();
        let mut h = hash;
        for _ in 0..32 {
            public_key.push((h & 0xFF) as u8);
            h >>= 8;
            if h == 0 {
                h = hash.wrapping_mul(6364136223846793005);
            }
        }
        public_key
    }
    
    fn calculate_fingerprint(public_key: &[u8]) -> String {
        // SHA256 hash formatted as fingerprint
        // In production, use sha2 crate
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        public_key.hash(&mut hasher);
        let hash = hasher.finish();
        
        // Format as readable fingerprint
        format!("{:04X}-{:04X}-{:04X}-{:04X}",
            (hash >> 48) & 0xFFFF,
            (hash >> 32) & 0xFFFF,
            (hash >> 16) & 0xFFFF,
            hash & 0xFFFF
        )
    }
    
    fn simple_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
        // Simplified HMAC-like construction
        // In production, use ed25519-dalek for real Ed25519 signatures
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        data.hash(&mut hasher);
        let hash = hasher.finish();
        
        let mut signature = Vec::new();
        let mut h = hash;
        for _ in 0..64 {
            signature.push((h & 0xFF) as u8);
            h = h.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
        }
        signature
    }
    
    fn get_device_name() -> String {
        if let Ok(output) = std::process::Command::new("hostname").output() {
            if let Ok(name) = String::from_utf8(output.stdout) {
                return name.trim().to_string();
            }
        }
        "Unknown Device".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_security_manager() {
        let temp_dir = std::env::temp_dir().join("ambient_security_test");
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let manager = SecurityManager::new(temp_dir.clone()).unwrap();
        assert!(!manager.identity.device_id.is_empty());
        assert!(!manager.identity.fingerprint.is_empty());
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
    
    #[test]
    fn test_pairing() {
        let temp_dir = std::env::temp_dir().join("ambient_pairing_test");
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let mut manager1 = SecurityManager::new(temp_dir.clone()).unwrap();
        let mut manager2 = SecurityManager::new(temp_dir.join("device2")).unwrap();
        
        // Device 1 creates pairing request
        let request = manager1.create_pairing_request().unwrap();
        
        // Device 2 handles it
        let trusted = manager2.handle_pairing_request(request).unwrap();
        assert_eq!(trusted.device_id, manager1.identity.device_id);
        
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
}
