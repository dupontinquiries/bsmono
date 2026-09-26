// PhotoSync Daemon - Monitors directories and processes photos
use photosync::PhotoSyncDB;
use std::path::{Path, PathBuf};
use std::time::Duration;
use notify::{Watcher, RecursiveMode, Event};
use walkdir::WalkDir;
use clap::Parser;
use anyhow::Result;

#[derive(Parser)]
#[command(name = "photosync-daemon")]
#[command(about = "PhotoSync background daemon for compression and deduplication")]
struct Args {
    /// Directory to watch for new photos
    #[arg(short, long)]
    watch_dir: PathBuf,
    
    /// Database path
    #[arg(short, long, default_value = "/var/lib/photosync/photosync.db")]
    db_path: PathBuf,
    
    /// Block storage directory
    #[arg(short, long, default_value = "/var/lib/photosync/blocks")]
    block_store: PathBuf,
    
    /// Device ID for tracking
    #[arg(short = 'i', long)]
    device_id: String,
    
    /// Thumbnail size (width)
    #[arg(short = 't', long, default_value = "256")]
    thumbnail_size: u32,
    
    /// Process existing files on startup
    #[arg(short, long)]
    process_existing: bool,
}

fn is_image_file(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_lowercase();
        matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "heic" | "raw" | "cr2" | "nef" | "arw" | "dng")
    } else {
        false
    }
}

/// The whole import is `PhotoSyncDB::import_photo`, in the library where it
/// is tested; this only logs.
fn process_image(
    db: &mut PhotoSyncDB,
    image_path: &Path,
    device_id: &str,
    thumbnail_size: u32,
) -> Result<()> {
    println!("Processing: {}", image_path.display());
    db.import_photo(image_path, device_id, thumbnail_size).map_err(|e| anyhow::anyhow!(e))
}

fn process_directory(
    db: &mut PhotoSyncDB,
    dir: &Path,
    device_id: &str,
    thumbnail_size: u32,
) -> Result<()> {
    let mut count = 0;
    
    for entry in WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() && is_image_file(path) {
            if let Err(e) = process_image(db, path, device_id, thumbnail_size) {
                eprintln!("Error processing {}: {}", path.display(), e);
            } else {
                count += 1;
            }
        }
    }
    
    println!("Processed {} images", count);
    Ok(())
}

// Not async: the loop below blocks on a std channel and nothing awaits. It was
// `#[tokio::main]` with tokio an optional dependency no default feature
// enables, so the daemon did not build at all without `websocket-sync`.
fn main() -> Result<()> {
    let args = Args::parse();
    
    // Ensure directories exist
    std::fs::create_dir_all(&args.db_path.parent().unwrap())?;
    std::fs::create_dir_all(&args.block_store)?;
    std::fs::create_dir_all(&args.watch_dir)?;
    
    let mut db = PhotoSyncDB::new(&args.db_path, &args.block_store).map_err(|e| anyhow::anyhow!(e))?;
    
    // Process existing files if requested
    if args.process_existing {
        println!("Processing existing files...");
        process_directory(&mut db, &args.watch_dir, &args.device_id, args.thumbnail_size)?;
        
        let stats = db.get_storage_stats().map_err(|e| anyhow::anyhow!(e))?;
        println!("\nStorage Statistics:");
        println!("  Total size: {} MB", stats.total_size / (1024 * 1024));
        println!("  Compressed: {} MB", stats.compressed_size / (1024 * 1024));
        println!("  Saved: {} MB", stats.space_saved() / (1024 * 1024));
        println!("  Compression: {:.1}%", stats.compression_ratio());
        println!("  Blocks: {}", stats.block_count);
    }
    
    // Set up file watcher
    println!("Watching directory: {}", args.watch_dir.display());
    
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(&args.watch_dir, RecursiveMode::Recursive)?;
    
    // Process events
    loop {
        match rx.recv() {
            Ok(Ok(Event { paths, .. })) => {
                for path in paths {
                    if path.is_file() && is_image_file(&path) {
                        if let Err(e) = process_image(&mut db, &path, &args.device_id, args.thumbnail_size) {
                            eprintln!("Error processing {}: {}", path.display(), e);
                        }
                    }
                }
            }
            Ok(Err(e)) => eprintln!("Watch error: {}", e),
            Err(e) => eprintln!("Channel error: {}", e),
        }
    }
}
