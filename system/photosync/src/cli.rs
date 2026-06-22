// PhotoSync CLI - Command-line interface for managing photo sync
use photosync::{PhotoSyncDB, StorageStats};
use std::path::PathBuf;
use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name = "photosync-cli")]
#[command(about = "PhotoSync command-line management tool")]
struct Cli {
    /// Database path
    #[arg(short, long, default_value = "/var/lib/photosync/photosync.db")]
    db_path: PathBuf,
    
    /// Block storage directory
    #[arg(short, long, default_value = "/var/lib/photosync/blocks")]
    block_store: PathBuf,
    
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show storage statistics
    Stats,
    
    /// Export a file from the deduplication store
    Export {
        /// File path in the database
        #[arg(short, long)]
        file: String,
        
        /// Output path
        #[arg(short, long)]
        output: PathBuf,
    },
    
    /// Show thumbnail for a file
    ShowThumbnail {
        /// File path or hash
        #[arg(short, long)]
        file: String,
        
        /// Output path for thumbnail
        #[arg(short, long)]
        output: PathBuf,
    },
    
    /// List all files in the database
    List {
        /// Filter by device ID
        #[arg(short, long)]
        device: Option<String>,
    },
    
    /// Verify integrity of stored blocks
    Verify,
    
    /// Garbage collect unreferenced blocks
    Gc {
        /// Dry run (don't delete)
        #[arg(short, long)]
        dry_run: bool,
    },
}

fn format_size(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;
    
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    
    format!("{:.2} {}", size, UNITS[unit_index])
}

fn show_stats(db: &PhotoSyncDB) -> Result<()> {
    let stats = db.get_storage_stats()?;
    
    println!("PhotoSync Storage Statistics");
    println!("============================");
    println!("Total original size:    {}", format_size(stats.total_size));
    println!("Compressed size:        {}", format_size(stats.compressed_size));
    println!("Space saved:            {}", format_size(stats.space_saved()));
    println!("Compression ratio:      {:.1}%", stats.compression_ratio());
    println!("Number of blocks:       {}", stats.block_count);
    println!("Average block size:     {}", 
             if stats.block_count > 0 {
                 format_size(stats.total_size / stats.block_count)
             } else {
                 "N/A".to_string()
             });
    println!("Deduplication savings:  {}", 
             format_size(stats.total_size - stats.compressed_size));
    
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    let db = PhotoSyncDB::new(&cli.db_path, &cli.block_store)?;
    
    match cli.command {
        Commands::Stats => {
            show_stats(&db)?;
        }
        
        Commands::Export { file, output } => {
            // Implementation would query the database for chunk hashes
            // and reconstruct the file
            println!("Exporting {} to {}", file, output.display());
            println!("(Export functionality requires database query implementation)");
        }
        
        Commands::ShowThumbnail { file, output } => {
            if let Some(thumb_data) = db.get_thumbnail(&file)? {
                std::fs::write(&output, thumb_data)?;
                println!("Thumbnail saved to {}", output.display());
            } else {
                println!("No thumbnail found for {}", file);
            }
        }
        
        Commands::List { device } => {
            println!("Listing files...");
            if let Some(dev) = device {
                println!("Filtered by device: {}", dev);
            }
            println!("(List functionality requires database query implementation)");
        }
        
        Commands::Verify => {
            println!("Verifying block integrity...");
            println!("(Verify functionality requires implementation)");
        }
        
        Commands::Gc { dry_run } => {
            println!("Running garbage collection...");
            if dry_run {
                println!("DRY RUN - no files will be deleted");
            }
            println!("(GC functionality requires implementation)");
        }
    }
    
    Ok(())
}
