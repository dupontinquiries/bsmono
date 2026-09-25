# PhotoSync System Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        PhotoSync Ecosystem                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌───────────────┐         ┌───────────────┐                    │
│  │ Linux Laptop  │◄───────►│ Linux Phone   │                    │
│  │  (Fedora)     │         │  (PostmarketOS)                    │
│  └───────┬───────┘         └───────┬───────┘                    │
│          │                         │                             │
│          │    Syncthing (TLS)      │                             │
│          │◄────────────────────────┤                             │
│          │                         │                             │
│          ▼                         ▼                             │
│  ┌───────────────┐         ┌───────────────┐                    │
│  │ Home Server   │         │ Cloud Backup  │                    │
│  │  (Alpine)     │         │  (Optional)   │                    │
│  └───────────────┘         └───────────────┘                    │
└─────────────────────────────────────────────────────────────────┘
```

## Component Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      PhotoSync Daemon Process                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌──────────────┐                                                │
│  │ File Watcher │ (inotify/notify)                               │
│  └──────┬───────┘                                                │
│         │ New file detected                                      │
│         ▼                                                         │
│  ┌──────────────────────────────────────────────────────┐        │
│  │              Image Processing Pipeline               │        │
│  ├──────────────────────────────────────────────────────┤        │
│  │                                                       │        │
│  │  1. Thumbnail Generation                             │        │
│  │     ├─ Resize to 256x256                             │        │
│  │     ├─ JPEG compression (85% quality)                │        │
│  │     └─ Store in SQLite                               │        │
│  │                                                       │        │
│  │  2. Chunking                                         │        │
│  │     ├─ Read file in 1 MiB blocks                     │        │
│  │     └─ Fixed-size chunking (or content-defined)      │        │
│  │                                                       │        │
│  │  3. Hashing                                          │        │
│  │     ├─ SHA-256 per chunk                             │        │
│  │     └─ Check for duplicates                          │        │
│  │                                                       │        │
│  │  4. Compression                                      │        │
│  │     ├─ zstd level 19 (or Brotli)                     │        │
│  │     ├─ Only compress new blocks                      │        │
│  │     └─ Store compressed data                         │        │
│  │                                                       │        │
│  │  5. Database Update                                  │        │
│  │     ├─ Register file metadata                        │        │
│  │     ├─ Link to block hashes                          │        │
│  │     └─ Update reference counts                       │        │
│  │                                                       │        │
│  └──────────────────────────────────────────────────────┘        │
│                                                                   │
└─────────────────────────────────────────────────────────────────┘
```

## Data Flow

```
┌──────────────┐
│   Camera     │
│  (Phone)     │
└──────┬───────┘
       │
       │ Save photo
       ▼
┌──────────────────┐
│  ~/Pictures/     │
│   PhotoSync/     │
└──────┬───────────┘
       │
       │ inotify event
       ▼
┌──────────────────┐
│  PhotoSync       │
│  Daemon          │
└──────┬───────────┘
       │
       ├─────────────────────┐
       │                     │
       ▼                     ▼
┌──────────────┐      ┌──────────────┐
│  Thumbnail   │      │  Chunking    │
│  Generation  │      │  & Hashing   │
└──────┬───────┘      └──────┬───────┘
       │                     │
       │                     ▼
       │              ┌──────────────┐
       │              │ Deduplication│
       │              │    Check     │
       │              └──────┬───────┘
       │                     │
       │                     ├─ New block ─┐
       │                     │             │
       │                     ▼             ▼
       │              ┌──────────────┐ ┌─────────┐
       │              │ Compression  │ │ Skip    │
       │              │  (zstd -19)  │ │(exists) │
       │              └──────┬───────┘ └────┬────┘
       │                     │              │
       │                     ▼              │
       │              ┌──────────────┐      │
       │              │ Block Store  │      │
       │              │    /blocks/  │      │
       │              └──────┬───────┘      │
       │                     │              │
       └─────────────────────┴──────────────┘
                             │
                             ▼
                      ┌──────────────┐
                      │   SQLite DB  │
                      │  - files     │
                      │  - blocks    │
                      │  - thumbnails│
                      └──────┬───────┘
                             │
                             ▼
                      ┌──────────────┐
                      │  Syncthing   │
                      │   Watches    │
                      └──────┬───────┘
                             │
                             ▼
                      ┌──────────────┐
                      │  Remote      │
                      │  Devices     │
                      └──────────────┘
```

## Database Schema

```sql
-- Blocks table: Stores compressed data blocks
CREATE TABLE blocks (
    hash TEXT PRIMARY KEY,           -- SHA-256 of original data
    size INTEGER NOT NULL,            -- Original size in bytes
    compressed_size INTEGER NOT NULL, -- Compressed size in bytes
    ref_count INTEGER NOT NULL,       -- Number of files referencing
    created_at INTEGER NOT NULL       -- Unix timestamp
);

-- Files table: Metadata for each photo
CREATE TABLE files (
    path TEXT PRIMARY KEY,            -- Full file path
    size INTEGER NOT NULL,            -- Original file size
    chunks TEXT NOT NULL,             -- JSON array of block hashes
    device_id TEXT NOT NULL,          -- Device identifier
    modified_at INTEGER NOT NULL,     -- Last modification time
    thumbnail_hash TEXT               -- Reference to thumbnail
);

-- Thumbnails table: Cached preview images
CREATE TABLE thumbnails (
    hash TEXT PRIMARY KEY,            -- Hash of thumbnail data
    data BLOB NOT NULL,               -- Compressed thumbnail
    width INTEGER NOT NULL,           -- Thumbnail width
    height INTEGER NOT NULL           -- Thumbnail height
);

-- Indexes for performance
CREATE INDEX idx_files_device ON files(device_id);
CREATE INDEX idx_blocks_refcount ON blocks(ref_count);
```

## File System Layout

```
/var/lib/photosync/
├── photosync.db              # SQLite database
├── blocks/                    # Block storage
│   ├── a1b2c3d4...           # Compressed block (SHA-256 hash)
│   ├── e5f6g7h8...           # Compressed block
│   └── ...                   # More blocks
└── backups/                   # Optional database backups
    └── photosync.db.YYYYMMDD

~/Pictures/PhotoSync/          # Synced directory
├── 2026-02-27/               # Date-organized
│   ├── IMG_0001.jpg
│   ├── IMG_0002.jpg
│   └── ...
├── Family/                    # Shared folder
│   └── ...
├── Personal/                  # Private folder
│   └── ...
└── .stignore                  # Syncthing ignore patterns

~/.config/photosync/
└── config.toml               # User configuration

~/.config/syncthing/
└── config.xml                # Syncthing configuration
```

## Network Communication

```
┌─────────────┐              ┌─────────────┐
│  Device A   │              │  Device B   │
│             │              │             │
│  Syncthing  │◄────TLS─────►│  Syncthing  │
│   (22000)   │              │   (22000)   │
└──────┬──────┘              └──────┬──────┘
       │                            │
       │ Local files                │ Local files
       ▼                            ▼
┌──────────────┐            ┌──────────────┐
│ PhotoSync    │            │ PhotoSync    │
│ Block Store  │            │ Block Store  │
└──────────────┘            └──────────────┘

Communication Protocol:
1. Device A creates/modifies file
2. PhotoSync daemon processes (chunk, compress, dedup)
3. Syncthing detects changes in watch directory
4. Syncthing sends ONLY new/changed blocks to Device B
5. Device B's Syncthing receives blocks
6. PhotoSync on Device B decompresses as needed
7. Both sides maintain local SQLite DB for metadata
```

## Process Lifecycle

```
System Boot
    │
    ▼
systemd/OpenRC starts photosync.service
    │
    ├─ Set process priority (nice 10)
    ├─ Set I/O priority (idle)
    └─ Start daemon
       │
       ▼
    Initialize
       │
       ├─ Open SQLite database (WAL mode)
       ├─ Check block store directory
       ├─ Set up file watcher
       └─ Process existing files (if --process-existing)
          │
          ▼
       Main Loop
          │
          ├─ Wait for inotify events
          ├─ Process new/modified files
          ├─ Update database
          └─ Log statistics
             │
             ▼
          On Signal
             │
             ├─ SIGTERM: Graceful shutdown
             ├─ SIGHUP: Reload config
             └─ SIGUSR1: Print statistics

On Crash/Error
    │
    └─ systemd auto-restart (after 10s delay)
```

## Performance Characteristics

### Compression Ratios
- **JPEG photos**: 10-20% reduction (already compressed)
- **RAW photos**: 40-60% reduction
- **PNG screenshots**: 50-70% reduction
- **Burst photos** (with dedup): 80-95% reduction

### Processing Speed
- **Thumbnail generation**: ~5-10ms per image
- **Chunking**: ~50-100 MB/s
- **Hashing (SHA-256)**: ~500 MB/s
- **Compression (zstd -19)**: ~20-40 MB/s
- **Decompression (zstd)**: ~200-400 MB/s

### Resource Usage
- **CPU**: Minimal (nice 10, idle I/O)
- **RAM**: ~50-100 MB per daemon
- **Disk**: 1x compressed + SQLite overhead (~5%)
- **Network**: Only delta changes (block-level)

## Security Model

```
┌─────────────────────────────────────┐
│        Security Layers              │
├─────────────────────────────────────┤
│                                     │
│  1. Transport (Syncthing)           │
│     ├─ TLS 1.3                      │
│     ├─ Self-signed certificates     │
│     └─ Device ID verification       │
│                                     │
│  2. Access Control                  │
│     ├─ Device ID whitelist          │
│     ├─ Folder-level permissions     │
│     └─ File versioning              │
│                                     │
│  3. Data at Rest (Optional)         │
│     ├─ LUKS encryption              │
│     ├─ File permissions (700)       │
│     └─ SQLite encryption            │
│                                     │
│  4. Integrity                       │
│     ├─ SHA-256 checksums            │
│     ├─ Block verification           │
│     └─ Database ACID properties     │
│                                     │
└─────────────────────────────────────┘
```

## Update: a gallery UI and a native P2P sync path (new, not yet wired to the daemon)

Everything above describes the original daemon: process files on disk,
lean on Syncthing for transport, expose nothing to look at. Three pieces
added since have not touched that daemon and do not replace Syncthing
here — they're new surfaces alongside it:

**A gallery.** `src/album.rs` (headless: grouping, albums, selection,
sharing — an album is a set of paths, never a copy) and
`src/gallery_grid.rs` (layout math: column count, cell placement,
virtualized visible-range) are pure logic with no renderer. Reading is via
`PhotoSyncDB::connection()` + `album::load()`, which returns a `Library`
complete with albums from the daemon's own `files`/`albums` tables — this
accessor didn't exist for a while, which made `album::load` unreachable
from anything holding a `PhotoSyncDB` (every real caller: daemon, sync
server, CLI). `crates/photosync-egui/` is the renderer: an egui shell on
the fleet's shared `uikit-egui` kit, styled after Fotema's camera-roll
shape — Library/Albums/Settings as ordinary rail destinations (Settings
is a ordinary tab, not a separate window the way GTK/libadwaita apps
place it). Thumbnails are the real pictures: `thumbs.rs` decodes the
visible tiles' files off the UI thread (newest request first, a
byte-bounded LRU in memory, no cache on disk to grow over time) and the
grid draws them cropped to fill; the procedural styles
(`settings::ThumbStyle`) remain as the placeholder while one loads or when a
file does not decode. The binary opens the daemon's library READ-ONLY
(`App::from_path`: `--db PATH`, `$PHOTOSYNC_DB`, or the daemon's default);
`--demo`, or a library that cannot be opened, shows `App::demo()` with a
status line saying so.

**Per-device permissions.** `Permission` (`ReadOnly < Contribute <
FullSync`, real `Ord`-based capability checks) plus a `permissions` table
keyed on `device_id`, with `set_permission`/`permission_for`/`authorize`
on `PhotoSyncDB`. Deliberately not threaded into `register_file` or
`garbage_collect` themselves — `authorize()` is a query a future sync
server's request handler checks before acting, so the CLI's local,
single-device use is never gated by a system it never asked for. Nothing
calls `authorize()` yet; `sync_server.rs` still accepts writes ungated.

**A native P2P sync path, built but not connected to anything real.**
`src/peers.rs`: sync-partner discovery on `dht-kit` (the fleet's
wire-format-only DHT primitives — no transport, no routing table by
design). Derives a library id into an `InfoHash`, announces this device's
`Permission` offer, finds other announced devices. `src/chunk_sync.rs`: a
durable per-(library, peer) sync channel built on `snapclone-group`'s
Merkle DAG + anti-entropy engine + delivery tracker — reused for
something it wasn't written for (ephemeral group snaps → durable photo
chunks). One DAG node per chunk, not per file, so an interrupted transfer
resumes instead of restarting a large file from zero; the DAG/delivery
state is meant to persist across many short connections
(`ChunkSync::begin_connection` resets in-flight request bookkeeping for a
fresh connection while keeping every chunk already admitted — found and
fixed a real stall in `snapclone-group`'s engine where an interrupted
request never got retried otherwise). Chunk bytes themselves stay in this
daemon's own block store, keyed by the same hash `chunk_and_store`
already computes — never only in the DHT/DAG layer, which is designed to
expire.

None of this has a transport wired in (no NAT traversal, no encrypted
handshake — `chunk_sync.rs`'s own doc names `snapclone-group`'s sealed
`entry.rs`/crypto as the natural next layer, deliberately not pulled in
yet). It's the discovery-and-diffing half of a sync protocol, sitting
next to — not replacing — the Syncthing-based transport this document
describes above, until a decision is made about which one this app
actually ships with.
