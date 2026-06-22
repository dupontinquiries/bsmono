// WebSocket Sync Server - Real-time photo synchronization
// Supports multiple clients with efficient block-level sync

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use futures_util::{StreamExt, SinkExt};
use serde::{Serialize, Deserialize};
use crate::{PhotoSyncDB, DeduplicationConfig, ChunkingStrategy, HashAlgorithm};

// WebSocket message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum SyncMessage {
    // Client → Server
    Register {
        device_id: String,
        device_name: String,
    },
    RequestBlocks {
        hashes: Vec<String>,
    },
    UploadFile {
        path: String,
        size: i64,
        chunks: Vec<String>,
        metadata: Option<String>,
    },
    UploadBlock {
        hash: String,
        data: Vec<u8>,  // Compressed block data
    },
    ListFiles {
        device_id: Option<String>,
    },
    
    // Server → Client
    Registered {
        client_id: String,
    },
    BlocksAvailable {
        hashes: Vec<String>,
    },
    BlocksMissing {
        hashes: Vec<String>,
    },
    BlockData {
        hash: String,
        data: Vec<u8>,
    },
    FileList {
        files: Vec<FileInfo>,
    },
    Error {
        message: String,
    },
    
    // Bidirectional
    Heartbeat,
    FileUpdated {
        path: String,
        device_id: String,
        chunks: Vec<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct FileInfo {
    path: String,
    size: i64,
    device_id: String,
    modified_at: i64,
    chunk_count: i64,
}

// Shared state
struct SyncServer {
    db: Arc<Mutex<PhotoSyncDB>>,
    clients: Arc<Mutex<HashMap<String, Client>>>,
}

struct Client {
    device_id: String,
    device_name: String,
}

impl SyncServer {
    fn new(db_path: &Path, block_store: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let config = DeduplicationConfig {
            chunking_strategy: ChunkingStrategy::ContentDefined,
            hash_algorithm: HashAlgorithm::XXH64,
            ..Default::default()
        };
        
        let db = PhotoSyncDB::with_config(db_path, block_store, config)?;
        
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            clients: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    
    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        println!("[SyncServer] Listening on: {}", addr);
        
        while let Ok((stream, addr)) = listener.accept().await {
            println!("[SyncServer] New connection from: {}", addr);
            
            let db = self.db.clone();
            let clients = self.clients.clone();
            
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(stream, db, clients).await {
                    eprintln!("[SyncServer] Error handling client: {}", e);
                }
            });
        }
        
        Ok(())
    }
    
    async fn handle_client(
        stream: tokio::net::TcpStream,
        db: Arc<Mutex<PhotoSyncDB>>,
        clients: Arc<Mutex<HashMap<String, Client>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let ws_stream = accept_async(stream).await?;
        let (mut write, mut read) = ws_stream.split();
        
        let mut client_id: Option<String> = None;
        
        while let Some(msg) = read.next().await {
            let msg = msg?;
            
            if msg.is_binary() || msg.is_text() {
                let text = msg.to_text()?;
                let sync_msg: SyncMessage = serde_json::from_str(text)?;
                
                match sync_msg {
                    SyncMessage::Register { device_id, device_name } => {
                        println!("[SyncServer] Device registered: {} ({})", device_id, device_name);
                        
                        let id = device_id.clone();
                        client_id = Some(id.clone());
                        
                        clients.lock().unwrap().insert(id.clone(), Client {
                            device_id: device_id.clone(),
                            device_name: device_name.clone(),
                        });
                        
                        let response = SyncMessage::Registered {
                            client_id: id,
                        };
                        
                        let json = serde_json::to_string(&response)?;
                        write.send(Message::Text(json)).await?;
                    }
                    
                    SyncMessage::RequestBlocks { hashes } => {
                        println!("[SyncServer] Block request for {} hashes", hashes.len());
                        
                        let db_lock = db.lock().unwrap();
                        let mut available = Vec::new();
                        let mut missing = Vec::new();
                        
                        for hash in hashes {
                            if db_lock.block_exists(&hash).unwrap_or(false) {
                                available.push(hash);
                            } else {
                                missing.push(hash);
                            }
                        }
                        drop(db_lock);
                        
                        if !available.is_empty() {
                            let response = SyncMessage::BlocksAvailable { hashes: available };
                            let json = serde_json::to_string(&response)?;
                            write.send(Message::Text(json)).await?;
                        }
                        
                        if !missing.is_empty() {
                            let response = SyncMessage::BlocksMissing { hashes: missing };
                            let json = serde_json::to_string(&response)?;
                            write.send(Message::Text(json)).await?;
                        }
                    }
                    
                    SyncMessage::UploadBlock { hash, data } => {
                        println!("[SyncServer] Receiving block: {}", hash);
                        
                        // Store block (data is already compressed)
                        let block_path = db.lock().unwrap().block_store.join(hash.replace(":", "_"));
                        std::fs::write(&block_path, &data)?;
                        
                        // Update database (data is already compressed, so size = compressed_size)
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_secs() as i64;
                        
                        let hash_algorithm = hash.split(':').next().unwrap_or("unknown");
                        
                        db.lock().unwrap().conn.execute(
                            "INSERT OR IGNORE INTO blocks 
                             (hash, hash_algorithm, size, compressed_size, ref_count, created_at, last_accessed) 
                             VALUES (?, ?, ?, ?, 1, ?, ?)",
                            params![hash, hash_algorithm, data.len() as i64, data.len() as i64, timestamp, timestamp],
                        )?;
                    }
                    
                    SyncMessage::UploadFile { path, size, chunks, metadata } => {
                        println!("[SyncServer] File upload: {}", path);
                        
                        if let Some(ref device) = client_id {
                            let mut db_lock = db.lock().unwrap();
                            db_lock.register_file(
                                &path,
                                size,
                                &chunks,
                                device,
                                metadata.as_deref(),
                            )?;
                        }
                        
                        // Notify other clients
                        if let Some(ref device) = client_id {
                            let notification = SyncMessage::FileUpdated {
                                path: path.clone(),
                                device_id: device.clone(),
                                chunks,
                            };
                            
                            // TODO: Broadcast to other clients
                        }
                    }
                    
                    SyncMessage::ListFiles { device_id } => {
                        println!("[SyncServer] List files request");
                        
                        // Query files from database
                        // TODO: Implement file listing
                        
                        let response = SyncMessage::FileList {
                            files: Vec::new(),  // TODO
                        };
                        
                        let json = serde_json::to_string(&response)?;
                        write.send(Message::Text(json)).await?;
                    }
                    
                    SyncMessage::Heartbeat => {
                        // Echo heartbeat
                        let response = SyncMessage::Heartbeat;
                        let json = serde_json::to_string(&response)?;
                        write.send(Message::Text(json)).await?;
                    }
                    
                    _ => {
                        eprintln!("[SyncServer] Unhandled message type");
                    }
                }
            }
        }
        
        // Remove client on disconnect
        if let Some(id) = client_id {
            clients.lock().unwrap().remove(&id);
            println!("[SyncServer] Client disconnected: {}", id);
        }
        
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = std::path::Path::new("/var/lib/photosync/photosync.db");
    let block_store = std::path::Path::new("/var/lib/photosync/blocks");
    
    let server = SyncServer::new(db_path, block_store)?;
    server.run("0.0.0.0:8765").await?;
    
    Ok(())
}
