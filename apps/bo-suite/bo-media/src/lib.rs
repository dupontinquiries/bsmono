// src/media/mod.rs
//
// The media library subsystem: a SQLite-backed catalog of local audio
// files with a 3-phase scanner (walk → parse → optional analysis), smart
// home cards, and a dedicated worker thread that owns the DB connection so
// the UI never blocks while loading thousands of tracks.
//
// Architecture:
//   - catalog.rs      schema + queries (the SQLite layer)
//   - parser.rs       filename → {artists, title, tags, album}
//   - scanner.rs      walk the music dir, upsert, parse, (analyze)
//   - smart_cards.rs  "resume", recent, derived home cards
//   - key_detection.rs  BPM/Camelot from bliss analysis (feature-gated)
//   - media_worker.rs  the thread + MediaCmd protocol
//
// Audio analysis (Phase 3, BPM/key via bliss-audio) is behind the
// `analysis` cargo feature because bliss pulls ffmpeg/symphonia and is a
// heavy, slow-compiling dependency. Without the feature the library still
// does everything except compute BPM/key — those columns stay null and
// key_detection returns stubs. Enable with:  cargo build --features analysis

pub mod catalog;
pub mod parser;
pub mod tag_reader;
pub mod tag_writer;
pub mod scanner;
pub mod smart_cards;
pub mod key_detection;
pub mod media_worker;
pub mod library;

pub use media_worker::{MediaCmd, run as run_media_worker};

/// Output messages from the media worker thread to the UI thread. Mirrors
/// the variants the worker/scanner emit (see result_message.rs notes in the
/// original pack). Strings are JSON payloads ready for QML consumption.
pub enum MediaResult {
    /// Live scan progress (phase, counts).
    ScanProgress(scanner::ScanProgress),
    /// Catalog totals for the library header.
    CatalogStats(catalog::CatalogStats),
    /// A list of TrackSummary as JSON, tagged with the context that asked
    /// for it (e.g. "recent", "artist:42", "bpm:120-128").
    TrackList { context: String, json: String },
    /// Artist or tag group counts as JSON, tagged with context.
    GroupList { context: String, json: String },
    /// Paginated counterpart to TrackList / GroupList. Carries the
    /// page's JSON plus enough metadata for the UI to know its
    /// position in the larger result set:
    ///   - `context`: same routing tag as TrackList ("artists",
    ///                "recent", "artist:42", ...)
    ///   - `json`:    serialized page contents (Vec<TrackSummary>
    ///                or Vec<GroupCount>, depending on context)
    ///   - `offset`:  0-indexed start of this page within the full
    ///                result. Echoes back what the request asked for
    ///                so the UI can dedup if a stale page arrives
    ///                after a newer one
    ///   - `total`:   total number of items in the full result.
    ///                Used to size scrollbars and decide when to
    ///                stop requesting more pages.
    Page { context: String, json: String, offset: i64, total: i64 },
    /// BPM band counts JSON: [{label, lo, hi, count}, ...]
    BpmBandCounts(String),
    /// Confirmed star rating for a track after a SetTrackRating write, so
    /// the UI can reconcile its optimistic update.
    TrackRating { track_id: i64, rating: i64 },
    /// Outcome of a WriteTagsToFile request (writing tags to the actual
    /// audio file). ok=false carries a human-readable failure reason.
    TagWriteResult { track_id: i64, ok: bool, message: String },
    /// Resume state for the home card.
    ResumeState(smart_cards::ResumeState),
    /// Current track BPM + camelot for the now-playing HUD chip.
    NowPlayingKeyBpm { bpm: Option<f32>, camelot: Option<String>, key_text: Option<String> },
    /// A track's file path + display metadata, resolved for playback.
    /// QtState wires the path into the local HTTP server and POSTs the
    /// resulting URL to the device.
    TrackForPlay { track_id: i64, path: String, title: String, artist: String, duration_secs: u32 },
    /// Persisted anticipator transition counts loaded from KV store.
    /// Empty string when the key didn't exist (fresh install).
    AnticipatorStateLoaded(String),
    /// Result of a LoadAppKv. `value` is the empty string when no such
    /// key existed; callers should treat that as "use the default".
    AppKvLoaded { key: String, value: String },
    /// Non-fatal error string.
    Error(String),

    // ── Playlists / queue / tags / history results ──
    /// A newly-created playlist; the UI uses the id to navigate.
    PlaylistCreated { id: i64, name: String },
    /// Full playlist list snapshot as JSON.
    PlaylistsList(String),
    /// Tracks inside one playlist as JSON, plus the playlist's id for routing.
    PlaylistTracks { playlist_id: i64, json: String },
    /// Current queue state as JSON.
    QueueState { device_url: String, json: String },
    /// List of devices with non-empty queues, as JSON
    /// (`[{device_url, item_count}, ...]`).
    QueueDevices(String),

    // ── Soundscape loop processor ───────────────────────────────────
    /// Progress update during loop search (0.0..1.0).
    SoundscapeProgress(f32),
    /// Loop search complete. JSON of LoopResult + output path + size.
    /// Example: {"output_path": "...", "size_bytes": 24576, "loop": {...}}
    SoundscapeDone(String),
    /// Loop search failed with a human-readable reason.
    SoundscapeFailed(String),

    // ── Stream mixer offline processor ─────────────────────────────
    /// Progress update during file mix (0.0..1.0).
    MixerProcessProgress(f32),
    /// File mix complete. JSON: {output_path, size_bytes, frames}.
    MixerProcessDone(String),
    /// File mix failed with a human-readable reason.
    MixerProcessFailed(String),
    /// Spectrum analysis complete. JSON: {input_path, points: [{hz, db}, ...]}.
    SpectrumDone(String),
    /// Spectrum analysis failed.
    SpectrumFailed(String),
    /// Tag list snapshot as JSON.
    TagsList(String),
    /// Tags attached to a single track as JSON.
    TrackTags { track_id: i64, json: String },
    /// Recent play history as JSON.
    HistoryList(String),
    /// Aggregate plays-per-hour stats as JSON. 24 entries, ordered by
    /// hour: [{"hour": 0, "count": 12}, {"hour": 1, "count": 0}, ...].
    HistoryStats(String),
    /// Rolling KPI summary as JSON (total_plays, distinct_tracks,
    /// top_track, top_artist, lookback_days).
    HistorySummary(String),
    /// Artist aggregation (top-by-plays or alpha) as JSON.
    ArtistsList { context: String, json: String },
    /// Album aggregation (top-by-plays or alpha) as JSON.
    AlbumsList { context: String, json: String },
}
