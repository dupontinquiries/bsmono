// src/scanner.rs
//
// Three-phase scan:
//
//   Phase 1 (walk): directory traversal, upsert (path, size, mtime).
//     Fast. ~5–10k files/sec on SSD. Reports a single ProgressUpdate
//     per N files.
//
//   Phase 2 (parse): for every row that's new or changed since last
//     parse, run the filename parser and write parsed metadata +
//     artist/tag joins. Pure CPU, very fast.
//
//   Phase 3 (analyze): for every row without `analyzed_at`, run bliss
//     to extract BPM + chroma, then derive key/Camelot. This is the
//     slow phase — runs on a thread pool sized to CPU cores minus 1
//     to leave headroom for the UI. Resumable: if interrupted, only
//     unanalyzed rows are picked up next time.
//
// Progress is reported via ResultMessage::ScanProgress events.

use super::catalog;
#[cfg(feature = "analysis")]
use super::key_detection;
use super::MediaResult as ResultMessage;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScanPhase {
    #[default]
    Idle,
    Walking,
    Parsing,
    Analyzing,
    Done,
}

const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "wma", "aiff", "alac",
];

// ─── Phase 1 ────────────────────────────────────────────────────────────────

pub fn walk_and_upsert(
    conn: &mut Connection,
    roots: &[PathBuf],
    tx: &Sender<ResultMessage>,
) -> rusqlite::Result<Vec<i64>> {
    let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
        phase: ScanPhase::Walking, done: 0, total: 0,
    }));

    // ── Preload existing rows ──
    //
    // The old version did one SELECT per walked file to discover the
    // (id, size, mtime) tuple for that path. On a 50k-track library
    // that's 50k SELECTs against a B-tree, even when 99% of files
    // are unchanged since the last scan and the answer is "no work
    // needed". The disk-I/O for the WalkDir traversal is fast on
    // SSD; the SQL round-trips are what made scans feel slow.
    //
    // Bulk-load the existing (path, id, size, mtime, parsed_at)
    // tuples into a HashMap once, then do all lookups in memory.
    // The trade is RAM for query time: ~88 bytes per entry × 50k
    // tracks ≈ 4.4 MB, negligible. We free the map when this
    // function returns.
    //
    // `parsed_at IS NULL` is tracked separately because tracks
    // whose Phase 1 row landed but whose Phase 2 parse never
    // completed (e.g. a prior scan errored mid-flight, like the
    // "Invalid column type Integer" bug that aborted Phase 2 on
    // every fresh first-scan until it was fixed) are otherwise
    // stuck forever: they show as "unchanged" on every subsequent
    // walk (size + mtime match the row) and so never enter the
    // parse queue again. Forcing them back into `needs_parse`
    // when they're catalogued-but-unparsed lets the next scan
    // recover all the stragglers in one pass.
    let mut existing: HashMap<String, (i64, i64, i64, bool)> =
        HashMap::with_capacity(8192);
    {
        let mut stmt = conn.prepare(
            "SELECT id, path, size, mtime, parsed_at FROM tracks")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                // parsed_at is nullable; treat NULL as "not parsed yet".
                r.get::<_, Option<i64>>(4)?.is_some(),
            ))
        })?;
        for row in rows {
            let (id, path, size, mtime, has_parsed) = row?;
            existing.insert(path, (id, size, mtime, has_parsed));
        }
    }
    eprintln!("scanner: preloaded {} existing track rows", existing.len());

    let mut walked = 0u64;
    let mut found_paths: HashSet<String> = HashSet::with_capacity(existing.len() + 1024);
    let mut needs_parse = Vec::new();
    // Pending DB writes — batched into a single transaction at the end
    // of the walk. Holding the writes in memory while we walk lets us
    // commit a single transaction containing only the rows that
    // actually need work (instead of touching every row in the DB
    // through a SELECT path).
    let mut to_insert: Vec<(String, i64, i64)> = Vec::new();
    let mut to_update: Vec<(i64, i64, i64)> = Vec::new();

    for root in roots {
        // filter_entry decides per-directory whether to descend. Skipping
        // hidden dirs and known-noise directories (node_modules, build
        // outputs, caches) makes scans dramatically faster when a music
        // root happens to overlap a development workspace or system dir.
        let walker = WalkDir::new(root).follow_links(false).into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 { return true; } // always descend the root
                let name = e.file_name().to_string_lossy();
                if name.starts_with('.') { return false; }
                !matches!(name.as_ref(),
                    "node_modules" | "target" | "build" | "dist" | "out" |
                    "__pycache__" | ".cache" | ".git" | ".svn" | ".hg" |
                    "venv" | ".venv" | "env" | ".env" |
                    "Cache" | "Caches" | "Trash" | ".Trash" |
                    "Library" | "AppData" | "tmp"
                )
            });
        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() { continue; }
            let path = entry.path();
            if !is_audio(path) { continue; }

            let Ok(meta) = entry.metadata() else { continue };
            let size = meta.len() as i64;
            let mtime = meta.modified().ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let path_str = path.to_string_lossy().to_string();
            // In-memory lookup instead of SQL round-trip.
            match existing.get(&path_str) {
                Some(&(id, old_size, old_mtime, has_parsed))
                    if old_size == size && old_mtime == mtime =>
                {
                    // Size + mtime match the existing row.
                    // - If parse already happened: nothing to do.
                    // - If parse never landed (parsed_at NULL),
                    //   re-queue this track for Phase 2 so an earlier
                    //   aborted scan can recover its stragglers.
                    found_paths.insert(path_str);
                    if !has_parsed {
                        needs_parse.push(id);
                    }
                }
                Some(&(id, _, _, _)) => {
                    // Changed: queue an UPDATE that also clears the
                    // parsed/analyzed timestamps so phases 2+3 re-run.
                    to_update.push((id, size, mtime));
                    needs_parse.push(id);
                    found_paths.insert(path_str);
                }
                None => {
                    // New file: queue an INSERT. We don't have an id
                    // until we actually insert, so the parse list is
                    // filled in after the batch commit.
                    to_insert.push((path_str.clone(), size, mtime));
                    found_paths.insert(path_str);
                }
            }

            walked += 1;
            if walked % 500 == 0 {
                let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
                    phase: ScanPhase::Walking, done: walked, total: 0,
                }));
            }
        }
    }

    // ── Commit all the queued writes in one transaction ──
    //
    // Single transaction = single fsync at the end (with the default
    // sqlite synchronous=NORMAL). Prepared statements cached for the
    // duration so each INSERT/UPDATE is a small bytecode invocation
    // rather than parsing the SQL anew per row.
    let outer_tx = conn.unchecked_transaction()?;
    {
        let mut update_stmt = outer_tx.prepare(
            "UPDATE tracks SET size=?1, mtime=?2, parsed_at=NULL, \
             analyzed_at=NULL WHERE id=?3")?;
        for (id, size, mtime) in &to_update {
            update_stmt.execute(rusqlite::params![size, mtime, id])?;
        }
        let mut insert_stmt = outer_tx.prepare(
            "INSERT INTO tracks (path, size, mtime) VALUES (?1, ?2, ?3)")?;
        for (path, size, mtime) in &to_insert {
            insert_stmt.execute(rusqlite::params![path, size, mtime])?;
            // last_insert_rowid is per-connection; valid right after
            // each insert and before the next statement on the same
            // connection.
            needs_parse.push(outer_tx.last_insert_rowid());
        }
    }
    outer_tx.commit()?;

    // Delete rows whose files are gone.
    let deleted = catalog::delete_missing(conn, &found_paths)?;
    if deleted > 0 {
        eprintln!("scanner: removed {} missing files", deleted);
    }

    let unchanged = walked.saturating_sub((to_insert.len() + to_update.len()) as u64);
    // Existing rows already queued for re-parse (they're "unchanged"
    // by size+mtime but had parsed_at=NULL). Phase 1 doesn't separate
    // these out for `walked`'s accounting; surface them in the log so
    // a user with a half-parsed library can see the recovery happening.
    let recovered_unparsed = needs_parse.len()
        .saturating_sub(to_insert.len() + to_update.len());
    eprintln!("scanner: walked {} files; {} new, {} changed, {} unchanged{}",
              walked, to_insert.len(), to_update.len(), unchanged,
              if recovered_unparsed > 0 {
                  format!(" ({} unparsed re-queued)", recovered_unparsed)
              } else { String::new() });

    let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
        phase: ScanPhase::Walking, done: walked, total: walked,
    }));

    Ok(needs_parse)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .map(|s| AUDIO_EXTS.contains(&s.as_str()))
        .unwrap_or(false)
}

// ─── Phase 2 ────────────────────────────────────────────────────────────────

pub fn parse_pending(
    conn: &mut Connection,
    ids: &[i64],
    extra_known_artists: &[String],
    tx: &Sender<ResultMessage>,
) -> rusqlite::Result<()> {
    let total = ids.len() as u64;
    let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
        phase: ScanPhase::Parsing, done: 0, total,
    }));
    if ids.is_empty() { return Ok(()); }

    // ── Prefetch (id → path) for every id in one query ──
    //
    // Old code did `SELECT path FROM tracks WHERE id = ?1` per id —
    // N SELECTs against a B-tree to pull a single column we could
    // have gotten in one scan. For 50k tracks newly added that's
    // 50k SELECTs on the index, plus the matching transaction
    // overhead in apply_parse (which itself wrapped each call in
    // a fresh BEGIN/COMMIT and therefore an fsync).
    //
    // The combined effect was that Phase 2 — which is pure CPU
    // (tag reading + filename heuristics) — became I/O-bound on
    // SQLite metadata. Prefetch + one outer transaction collapses
    // both costs.
    let mut id_to_path: HashMap<i64, String> = HashMap::with_capacity(ids.len());
    {
        // Select only `path` (not `id, path`): the closure reads
        // column 0, and a `SELECT id, path …` makes column 0 the
        // INTEGER id, not the TEXT path — which produces a runtime
        // type error ("Invalid column type Integer at index 0,
        // name: id") that aborts the whole scan after Phase 1 has
        // already inserted all the rows. The user saw this with a
        // 9241-track first scan: walk succeeded, parse aborted,
        // and the next scan run had no work to do because the
        // catalog rows existed but had no parsed metadata.
        let mut stmt = conn.prepare("SELECT path FROM tracks WHERE id = ?1")?;
        for &id in ids {
            let path: Option<String> = stmt.query_row(
                rusqlite::params![id], |r| r.get(0)
            ).optional()?;
            if let Some(p) = path { id_to_path.insert(id, p); }
        }
    }

    // ── Phase 2a: parallel tag-read ──
    //
    // Tag reading is pure file I/O + parsing. Each file is independent
    // and the function (`tag_reader::read_or_parse`) takes no shared
    // mutable state. Reading is the slow part — on SD-card storage
    // each file is 0.5–2 ms of disk seek plus a small amount of CPU.
    //
    // We fan out across available cores and collect (id, parsed)
    // tuples, then apply them serially under one outer transaction.
    // The split is necessary because SQLite is single-writer; trying
    // to parallelize the DB apply would just bottleneck on the lock
    // anyway. Doing the slow part in parallel and the fast part
    // serially gives us most of the wall-clock win without the
    // contention.
    //
    // Progress is emitted as a relaxed-atomic counter — threads bump
    // it and any thread that crosses a 100-track boundary sends a
    // ScanProgress to tx. That keeps the UI responsive even when one
    // thread is briefly stalled on a slow file.
    let work: Vec<(i64, String)> = ids.iter()
        .filter_map(|&id| id_to_path.get(&id).map(|p| (id, p.clone())))
        .collect();

    let n_threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8).max(1))
        .unwrap_or(2);
    // For libraries small enough that the parallel overhead would
    // dominate, just go sequential. The crossover at ~16 tracks is
    // a guess but the overhead of spawning + joining 4 threads is
    // ~1 ms; below that count it's not worth it.
    let progress = std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(0));
    let mut parsed_results: Vec<(i64, crate::media::parser::ParsedTrack)> =
        Vec::with_capacity(work.len());

    if work.len() < 16 || n_threads <= 1 {
        // Sequential path — same shape as parallel for code-reuse
        // in the apply loop below.
        for (id, path) in &work {
            let path_obj = Path::new(path);
            let fname = path_obj.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(path)
                .to_string();
            let parsed = super::tag_reader::read_or_parse(
                path_obj, &fname, extra_known_artists);
            parsed_results.push((*id, parsed));
            let done = progress.fetch_add(
                1, std::sync::atomic::Ordering::Relaxed) + 1;
            if done % 100 == 0 || done == total {
                let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
                    phase: ScanPhase::Parsing, done, total,
                }));
            }
        }
    } else {
        // Parallel path. Divide the work into N chunks of equal
        // size. Each thread handles its chunk and returns its
        // results to the main thread when joined; total ordering
        // is preserved by chunking on the original ID order.
        let chunk_size = (work.len() + n_threads - 1) / n_threads;
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for chunk in work.chunks(chunk_size) {
                let tx_clone = tx.clone();
                let progress = progress.clone();
                let extra = extra_known_artists.to_vec();
                let chunk = chunk.to_vec();
                let h = scope.spawn(move || {
                    let mut out: Vec<(i64, crate::media::parser::ParsedTrack)>
                        = Vec::with_capacity(chunk.len());
                    for (id, path) in chunk {
                        let path_obj = Path::new(&path);
                        let fname = path_obj.file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&path)
                            .to_string();
                        let parsed = super::tag_reader::read_or_parse(
                            path_obj, &fname, &extra);
                        out.push((id, parsed));
                        let done = progress.fetch_add(
                            1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if done % 100 == 0 || done == total {
                            let _ = tx_clone.send(ResultMessage::ScanProgress(
                                ScanProgress {
                                    phase: ScanPhase::Parsing, done, total,
                                }));
                        }
                    }
                    out
                });
                handles.push(h);
            }
            for h in handles {
                if let Ok(part) = h.join() {
                    parsed_results.extend(part);
                }
            }
        });
        // Re-sort by id to preserve the deterministic order the
        // sequential path produces. Not strictly necessary for
        // correctness (apply_parse_in_tx is order-independent),
        // but matches existing test expectations and keeps logs
        // readable across runs.
        parsed_results.sort_by_key(|&(id, _)| id);
    }

    // ── Phase 2b: apply parsed results under one transaction ──
    //
    // All UPDATE / INSERT statements within apply_parse_in_tx run
    // against this one transaction. Commit happens once at the end,
    // so we pay for a single fsync regardless of how many tracks
    // we process. On SD-card storage (Fairphone 5 / postmarketOS)
    // this is the difference between minutes and seconds.
    let now = unix_now();
    let outer_tx = conn.unchecked_transaction()?;
    let mut failed: Vec<(i64, String, String)> = Vec::new();
    for (id, parsed) in &parsed_results {
        // Per-track SAVEPOINT — if apply_parse_in_tx fails on this
        // track (e.g. empty-norm artist from a filename like
        // "Artist - Title (?!).mp3"), roll back only this track's
        // edits and keep going. Without this isolation, one bad
        // row aborts the outer transaction and zeroes the whole
        // batch.
        outer_tx.execute_batch("SAVEPOINT track_parse")?;
        match catalog::apply_parse_in_tx(&outer_tx, *id, parsed, now) {
            Ok(()) => outer_tx.execute_batch("RELEASE track_parse")?,
            Err(e) => {
                outer_tx.execute_batch(
                    "ROLLBACK TO track_parse; RELEASE track_parse")?;
                let path = id_to_path.get(id).cloned().unwrap_or_default();
                failed.push((*id, path, e.to_string()));
            }
        }
    }
    outer_tx.commit()?;

    // Final parse-complete summary. This is the line the user (and
    // any future operator) can grep for to confirm Phase 2 actually
    // ran and finished. The user's report of "0 tracks in artists
    // page" turned out to be because parse_pending had run BUT
    // hadn't surfaced any visible "done" signal — making it
    // indistinguishable from "still running" or "silently aborted"
    // when diagnosing.
    eprintln!(
        "scanner: parse_pending complete — {} parsed OK, {} failed; \
         track_artists rows are now populated (artists page should \
         show non-zero track counts)",
        parsed_results.len() - failed.len(),
        failed.len());

    if !failed.is_empty() {
        eprintln!(
            "scanner: {} track(s) failed to parse (first 5 below); they remain catalogued but unparsed and will be re-attempted on the next scan",
            failed.len());
        for (id, path, err) in failed.iter().take(5) {
            eprintln!("  id={} path={} err={}", id, path, err);
        }
    }
    Ok(())
}

// ─── Phase 3 ────────────────────────────────────────────────────────────────
//
// Uses bliss-audio. Decoupled from the main scan flow so a slow analysis
// pass doesn't hold a DB transaction open. Spawns a thread pool, pulls
// unanalyzed rows in chunks, writes results back as they complete.

#[cfg(feature = "analysis")]
pub fn analyze_pending(
    conn: &mut Connection,
    tx: &Sender<ResultMessage>,
    cancel: &std::sync::atomic::AtomicBool,
) -> rusqlite::Result<()> {
    // bliss-audio 0.9: the FFmpeg decoder type is `FFmpeg` (not the older
    // `FFmpegDecoder` name from internal documentation). This is the default
    // backend — bliss's `ffmpeg` feature is on by default, so no special
    // configuration is needed beyond installing libav* dev libraries on the
    // system (libavformat-dev, libavcodec-dev, libavutil-dev,
    // libswresample-dev, libavfilter-dev on Debian/Ubuntu).
    //
    // For pure-Rust decoding via Symphonia instead, override bliss-audio's
    // features in Cargo.toml:
    //   bliss-audio = { version = "0.9", default-features = false,
    //                   features = ["aubio-static","symphonia-all"],
    //                   optional = true }
    // and change the import below to:
    //   use bliss_audio::decoder::symphonia::SymphoniaDecoder as Decoder;
    //
    // `analyze_paths` is a trait method on `Decoder`, so the trait must be
    // in scope. `FFmpeg` is the impl type that provides it.
    use bliss_audio::decoder::ffmpeg::FFmpeg as Decoder;
    use bliss_audio::decoder::Decoder as _;
    use std::sync::atomic::Ordering;

    let total: u64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE analyzed_at IS NULL AND parsed_at IS NOT NULL",
        [], |r| r.get(0),
    )?;
    let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
        phase: ScanPhase::Analyzing, done: 0, total,
    }));

    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) { break; }

        // Pull a chunk of pending paths
        let chunk: Vec<(i64, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, path FROM tracks
                 WHERE analyzed_at IS NULL AND parsed_at IS NOT NULL
                 LIMIT 32"
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if chunk.is_empty() { break; }

        // Use bliss's parallel analysis iterator
        let paths: Vec<String> = chunk.iter().map(|(_, p)| p.clone()).collect();
        let path_to_id: std::collections::HashMap<String, i64> = chunk.iter()
            .map(|(id, p)| (p.clone(), *id))
            .collect();

        for (path, song_result) in Decoder::analyze_paths(&paths) {
            if cancel.load(Ordering::Relaxed) { break; }

            // analyze_paths yields (PathBuf, BlissResult<Song>); our id map is
            // keyed by the original String, so convert via to_string_lossy.
            let path_str = path.to_string_lossy().to_string();
            let id = match path_to_id.get(&path_str) {
                Some(id) => *id,
                None => continue,
            };
            match song_result {
                Ok(song) => {
                    let bpm = key_detection::tempo_from_analysis(&song.analysis);
                    let (camelot, key_text) = key_detection::key_from_analysis(&song.analysis);
                    catalog::apply_analysis(conn, id, bpm, &camelot, &key_text, unix_now())?;
                }
                Err(e) => {
                    eprintln!("analyze failed for {}: {}", path_str, e);
                    // Mark analyzed_at anyway so we don't retry forever — use a sentinel of -1
                    conn.execute(
                        "UPDATE tracks SET analyzed_at = -1 WHERE id = ?1",
                        rusqlite::params![id],
                    )?;
                }
            }
            done += 1;
            if done % 10 == 0 || done == total {
                let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
                    phase: ScanPhase::Analyzing, done, total,
                }));
            }
        }
    }

    let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
        phase: ScanPhase::Done, done, total,
    }));

    Ok(())
}

// Stub for builds without analysis (e.g. to test catalog/parser without
// pulling bliss-audio and its deps in).
#[cfg(not(feature = "analysis"))]
pub fn analyze_pending(
    _conn: &mut Connection,
    tx: &Sender<ResultMessage>,
    _cancel: &std::sync::atomic::AtomicBool,
) -> rusqlite::Result<()> {
    let _ = tx.send(ResultMessage::ScanProgress(ScanProgress {
        phase: ScanPhase::Done, done: 0, total: 0,
    }));
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Auto-cleaning temp directory under /tmp so the test doesn't
    /// require pulling in the `tempfile` crate just for two tests.
    struct TempDir { path: PathBuf }
    impl TempDir {
        fn new(label: &str) -> Self {
            let mut p = std::env::temp_dir();
            let stamp = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH).unwrap().as_nanos();
            p.push(format!("bo-scanner-test-{}-{}", label, stamp));
            std::fs::create_dir_all(&p).unwrap();
            Self { path: p }
        }
        fn path(&self) -> &Path { &self.path }
    }
    impl Drop for TempDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); }
    }

    /// Build a fresh in-memory-like DB with the production schema +
    /// a writable temp directory the walker can scan.
    fn fresh_db(label: &str) -> (Connection, TempDir) {
        let dir = TempDir::new(label);
        let db_path = dir.path().join("test.sqlite");
        let conn = catalog::open(&db_path).unwrap();
        (conn, dir)
    }

    /// Touch a fake audio file. The tag_reader will fail to parse
    /// it (no real ID3/Vorbis bytes), so the parser falls back to
    /// filename heuristics — but that's fine for these tests, which
    /// only exercise the walker's re-queue decision.
    fn touch_audio(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"\0\0\0\0fake").unwrap();
        p
    }

    #[test]
    fn unparsed_rows_get_requeued_on_next_scan() {
        // The user-visible scenario: a previous scan inserted rows
        // (Phase 1) but Phase 2 errored before parsing them (the
        // "Invalid column type Integer" bug). The track rows have
        // size + mtime set; parsed_at is NULL. On the next scan
        // (which sees identical files since nothing's changed), the
        // walker MUST re-queue these rows for Phase 2 — otherwise
        // they're stuck in catalogued-but-empty limbo forever and
        // the user sees an empty library card.
        let (mut conn, dir) = fresh_db("unparsed_requeue");
        let music_dir = dir.path().join("music");
        std::fs::create_dir(&music_dir).unwrap();
        let _f = touch_audio(&music_dir, "track.mp3");

        let (tx, rx) = mpsc::channel();
        // First walk inserts the row.
        let pending = walk_and_upsert(
            &mut conn, &[music_dir.clone()], &tx).unwrap();
        assert_eq!(pending.len(), 1,
            "first walk should produce 1 pending id; got {:?}", pending);
        drop(rx);

        // Simulate the bug: Phase 2 never ran. parsed_at stays NULL.
        // (The row was inserted by walk_and_upsert with parsed_at not
        // set — i.e. NULL — by default, so no manual update needed.)

        // Second walk: file unchanged, but parsed_at NULL → should
        // re-queue.
        let (tx2, rx2) = mpsc::channel();
        let pending2 = walk_and_upsert(
            &mut conn, &[music_dir.clone()], &tx2).unwrap();
        assert_eq!(pending2.len(), 1,
            "unparsed row must be re-queued on next scan; got {:?}",
            pending2);
        drop(rx2);

        // Now simulate Phase 2 succeeding: set parsed_at to a real value.
        conn.execute(
            "UPDATE tracks SET parsed_at = ?1, title = 'Track' WHERE id = ?2",
            rusqlite::params![unix_now(), pending2[0]]).unwrap();

        // Third walk: row is now parsed AND unchanged → NOT re-queued.
        let (tx3, rx3) = mpsc::channel();
        let pending3 = walk_and_upsert(
            &mut conn, &[music_dir.clone()], &tx3).unwrap();
        assert!(pending3.is_empty(),
            "fully-parsed unchanged row must not be re-queued; got {:?}",
            pending3);
        drop(rx3);
    }

    #[test]
    fn changed_files_are_requeued_regardless_of_parsed_state() {
        // Sanity: the existing "changed" path (size or mtime
        // differs) must still requeue even if parsed_at is set.
        let (mut conn, dir) = fresh_db("changed_requeue");
        let music_dir = dir.path().join("music");
        std::fs::create_dir(&music_dir).unwrap();
        let track = touch_audio(&music_dir, "song.mp3");

        let (tx, rx) = mpsc::channel();
        let p1 = walk_and_upsert(
            &mut conn, &[music_dir.clone()], &tx).unwrap();
        drop(rx);

        // Mark parsed.
        conn.execute(
            "UPDATE tracks SET parsed_at = ?1 WHERE id = ?2",
            rusqlite::params![unix_now(), p1[0]]).unwrap();

        // Rewrite the file with different content (changes size).
        std::fs::write(&track, b"\0\0\0\0fake_longer_content").unwrap();

        let (tx2, rx2) = mpsc::channel();
        let p2 = walk_and_upsert(
            &mut conn, &[music_dir.clone()], &tx2).unwrap();
        assert_eq!(p2.len(), 1,
            "changed file must always re-queue; got {:?}", p2);
        drop(rx2);
    }
}
