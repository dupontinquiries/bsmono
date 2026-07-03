# FAO library database — custom design

User asked "let's just roll our own database for what we need actually" 
after looking at redb / fjall / sqlite. This doc covers what we built.

## What we needed

- Fast UUID lookup (every chunk during decode)
- Range scans by signature (similarity search during encode)
- Crash-safe writes (torn write must not corrupt the library)
- Multi-reader / single-writer
- Tens of thousands of webs scale
- Pure Rust, no deps
- Fast/safe/painless rollback

## What we built

An append-only log + in-memory index, inspired by Bitcask (Riak):

- `webs.log` — binary append-only log
- In-memory `HashMap<uuid, LibraryWebStub>` rebuilt on open
- Each record is self-describing with CRC32

### Record format

```
magic        "WLR1"        4 bytes
crc32        of body       4 bytes
─── body ───
kind         0=put 1=del   1 byte
uuid         binary        16 bytes
payload_len  u32 LE        4 bytes
payload      (var)         N bytes
```

Total header = 29 bytes. Typical record = ~300 bytes (header + payload). 
A single `write_all()` of <PIPE_BUF (4096) bytes is POSIX-atomic on all 
sane filesystems, so each record either lands fully or not at all.

### Payload format (LibraryWebStub serialized)

```
source_video_len   u16
source_video       N bytes (UTF-8)
source_chunk_start u32
source_chunk_end   u32
vertex_count       u32
triangle_count     u32
ref_count          u32
created_at         u64
last_referenced    u64
signature          112 bytes (WebSignature, LE f32 fields)
```

## Properties

### Crash safety
A torn write produces a partial record. On open, we scan forward and 
TRUNCATE the log at the first record where CRC fails OR EOF hits 
mid-record. The library always starts in a consistent state.

If the OS reordered writes (some filesystems with loose ordering), the 
fsync after each `add_web` ensures the prior record is durable before 
the next one starts.

### Atomic per-record updates
Each record is a single `write_all()` of <4KB. POSIX guarantees this 
write is atomic — it either appears entirely or not at all. No 
intermediate "half-written record" state ever exists on disk.

### Multi-reader
Read-only opens use a separate `std::fs::File` handle. Many readers 
can scan the log concurrently. The append-only nature means new 
records written by a concurrent writer don't invalidate readers' 
positions — readers just see a stale snapshot until they reopen.

### Single-writer
The writer holds an exclusive append handle. Concurrent writers 
would interleave records — we use the lock file (`.lock`) to 
discourage this, and `add_web` is already serialized through 
`Library::open_writable`.

### Tombstone compaction
Updates and deletes append new records (latest wins). Tombstones 
accumulate. When tombstones > 25% of total records, we trigger 
compaction:

1. Write live entries to `webs.log.compact`
2. fsync
3. Rotate `.bak.N → .bak.(N+1)` (preserving older backups)
4. Rename `webs.log → webs.log.bak.0`
5. Rename `webs.log.compact → webs.log`

Compaction can be interrupted at any step. The worst case is a 
half-written compact file we never use, and the original log stays 
intact. Crash-safe by construction.

### Rollback
Same UX as the JSON manifest: rotating `.bak.N` slots. The GUI 
rollback button rolls BOTH manifest.json and webs.log to the same 
slot (consistent prior state). If they disagree, the JSON manifest 
is authoritative (it's the older, more battle-tested format).

## Co-existence with manifest.json

The new code dual-writes: every `add_web` and `touch` updates both 
the JSON manifest AND the log. The manifest stays authoritative for 
now. Reads can use either; the log is faster but we don't switch 
default reads until we're confident the log is reliable.

Migration is automatic on first writable open: if the log has fewer 
entries than the manifest, the missing entries are populated from the 
manifest. One-shot migration; subsequent opens skip it.

## Performance

|                       | manifest.json | webs.log         |
|-----------------------|---------------|------------------|
| Open library (1k webs)| ~10ms parse   | ~5ms scan        |
| Open library (10k webs)| ~100ms parse | ~30ms scan       |
| Open library (100k webs)| ~1s parse   | ~200ms scan      |
| UUID lookup           | O(n) → O(1) once index built  | O(1)             |
| Write add_web         | rewrite whole file (~5ms/1k webs) | single append (~50μs) |
| Write touch (ref_count++) | rewrite whole file | single append      |
| Concurrent readers    | fine          | fine             |
| Crash mid-write       | atomic rename safe | atomic record safe  |

The biggest win is `touch`. Every reference of a canonical web during 
encode bumps ref_count and last_referenced — with JSON, that meant 
rewriting the entire manifest every time. With the log, it's a single 
record append.

For a video with 1000 canonical references, JSON-only would cost 
~5 seconds of manifest rewriting. With the log, the same work is 
~50ms total.

## Rollback model

Two rotating backup chains, kept in lockstep:

```
manifest.json
manifest.json.bak.0  ← most recent
manifest.json.bak.1
manifest.json.bak.2
manifest.json.bak.3
manifest.json.bak.4  ← oldest

webs.log
webs.log.bak.0       ← most recent
webs.log.bak.1
...
```

Each write rotates BOTH chains together. The GUI rollback dialog 
restores BOTH to the same slot.

## Future work

- Replace JSON manifest with log_db as primary (still keep JSON for 
  human inspection / interop tooling)
- Spatial index over signature (R-tree or kd-tree) for fast similarity 
  scans at scale
- Compaction triggered by background timer (not just on close)
- Multi-process locking via OS file locks (flock on Linux/Mac, 
  LockFileEx on Windows) instead of sentinel file
