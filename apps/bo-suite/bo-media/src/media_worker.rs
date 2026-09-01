// src/media_worker.rs
//
// A dedicated worker thread for the media catalog. SQLite connections
// aren't Send-safe across async boundaries with the rusqlite default
// config, so we keep the connection on one thread and serialize all
// work through a mpsc channel.
//
// This is separate from the existing reqwest worker (which handles
// the B&O network calls) — there's no reason to multiplex catalog
// queries and HTTP requests on the same thread.
//
// Spawn at app startup:
//
//   let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
//   std::thread::spawn(move || media_worker::run(cmd_rx, result_tx, db_path, roots));

use super::catalog;
use super::library;
use super::MediaResult as ResultMessage;
use super::scanner;
use lofty::file::AudioFile;
use super::smart_cards;
use super::tag_writer;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Read a media file's duration in whole seconds via ffprobe. Used as a
/// fallback when lofty can't read it from the container header (some
/// Opus/streamed files). ffprobe ships with ffmpeg, which is already a
/// hard dependency for decode. Returns 0 if ffprobe is missing or fails,
/// so callers degrade exactly as before (no progress fill, free seeking).
fn ffprobe_duration_secs(path: &str) -> u32 {
    use std::process::Command;
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
                .map(|s| s.max(0.0) as u32)
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// Commands the QtState worker sends to the media thread. Several variants
/// are pre-staged for upcoming UI surfaces (resume / BPM-band navigator /
/// compatible-with-now matcher) and not yet invoked from QML, so we
/// suppress dead-code warnings on the enum as a whole rather than litter
/// it with per-variant attributes.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum MediaCmd {
    /// Re-scan all configured roots. Walk -> parse -> analyze.
    RescanLibrary,
    /// Walk + parse only — skip the slow analysis phase. Used on
    /// startup when the user just wants the catalog open quickly.
    QuickScan,
    /// Walk + parse a user-chosen set of roots (overrides the env-default).
    /// Empty = fall back to the configured defaults.
    ScanWithRoots(Vec<String>),
    /// Analyze rows that don't yet have BPM/key. Long-running, cancellable.
    AnalyzeBacklog,
    /// Stop any in-flight analysis cleanly.
    CancelAnalysis,

    /// "Smart cards" home view requests
    LoadHome,
    LoadResume,
    LoadRecent,
    LoadArtists,
    LoadTags,
    LoadBpmBandCounts,

    /// Paginated equivalents — preferred for any view that could
    /// hold thousands of rows. Each takes a 0-indexed offset and
    /// a limit; the worker responds with a ResultMessage that
    /// carries the same payload plus the total count and the
    /// offset that was requested, so the UI can render scrollbar
    /// extents and decide when to fetch the next page.
    ///
    /// Limit should be small enough that the QML ListView's
    /// delegate creation cost stays bounded (50-200 is a reasonable
    /// range). Offset can be anywhere in [0, total).
    LoadArtistsPage { offset: i64, limit: i64, sort: String },
    LoadRecentPage  { offset: i64, limit: i64 },
    LoadByArtistPage { artist_id: i64, offset: i64, limit: i64 },
    /// Every track in the library, paginated, ordered by title. Backs the
    /// "All Tracks" view so no track is ever unreachable regardless of how
    /// its filename parsed (missing-tracks fix).
    LoadAllTracksPage { offset: i64, limit: i64, sort: String },

    /// Drill-down list views
    LoadByArtist(i64),
    LoadByTag(i64),
    LoadByBpmBand(f32, f32),
    LoadByCamelot(String),
    LoadCompatibleNow { bpm: Option<f32>, camelot: Option<String> },
    /// Full-text search across title/artist/album. Returns a TrackList
    /// tagged with context "search:<query>".
    Search(String),

    /// Record that a track was played
    RecordPlay(i64),
    /// Look up bpm/camelot for a specific track (for the now-playing HUD chip)
    LoadKeyBpmForTrack(i64),
    /// Resolve a track id to its on-disk path + display metadata so QtState
    /// can expose it via the local HTTP server and ask the device to play it.
    ResolveForPlay(i64),
    /// Persist the anticipator's transition counts to the KV store.
    /// Called periodically + on app shutdown so learning survives restarts.
    SaveAnticipatorState(String),
    /// Load the persisted anticipator state on app startup. Returns
    /// AnticipatorStateLoaded with the JSON payload (or empty string
    /// if no prior state).
    LoadAnticipatorState,
    /// Generic KV upsert into the app_state table. Used by various
    /// subsystems (aggression policy, future settings) that just need a
    /// persistent string-to-string store. Key/value strings only;
    /// non-string payloads go through their own typed variants.
    SaveAppKv { key: String, value: String },
    /// Generic KV read from app_state. Returns AppKvLoaded with the
    /// payload (empty string when the key is missing).
    LoadAppKv { key: String },
    /// Set/clear a manual override for a track's display fields. Passing all
    /// `None` removes the override row so the parsed values reappear.
    SetOverride {
        track_id: i64,
        title:  Option<String>,
        artist: Option<String>,
        album:  Option<String>,
        year:   Option<u16>,
    },

    // ── Playlists ────────────────────────────────────────────────────
    /// Create a new playlist. Reply: PlaylistCreated.
    CreatePlaylist { name: String, description: String },
    /// Delete a playlist (cascades to its tracks). Reply: PlaylistsList.
    DeletePlaylist(i64),
    /// Rename + redescribe. Either arg empty leaves that field unchanged.
    /// Reply: PlaylistsList.
    UpdatePlaylistMeta { id: i64, name: String, description: String },
    /// Append a track to a playlist. Reply: PlaylistTracks.
    AddTrackToPlaylist { playlist_id: i64, track_id: i64 },
    /// Bulk add: append many tracks (skips dupes). Reply: PlaylistTracks.
    AddManyToPlaylist { playlist_id: i64, track_ids: Vec<i64> },
    /// Remove a track from a playlist. Reply: PlaylistTracks.
    RemoveTrackFromPlaylist { playlist_id: i64, track_id: i64 },
    /// Move a track to position `new_ord` in the playlist. Reply: PlaylistTracks.
    ReorderPlaylist { playlist_id: i64, track_id: i64, new_ord: i64 },
    /// Set a track's 0–5 star rating (durably, path-mirrored).
    SetTrackRating { track_id: i64, rating: i64 },
    /// Write metadata edits into the actual audio file's tags (portable,
    /// permanent). Distinct from SetOverride (which is a DB-only overlay).
    WriteTagsToFile {
        track_id: i64,
        title:  Option<String>,
        artist: Option<String>,
        album:  Option<String>,
        year:   Option<u32>,
    },
    /// List all playlists with track counts. Reply: PlaylistsList.
    ListPlaylists,
    /// Load all tracks in a playlist in order. Reply: PlaylistTracks.
    LoadPlaylistTracks(i64),

    // ── Queue ────────────────────────────────────────────────────────
    /// Push a track to the end of the queue. Reply: QueueState.
    QueuePush { track_id: i64, device_url: String },
    /// Insert at position `at` (0 = play next). Reply: QueueState.
    QueueInsert { track_id: i64, device_url: String, at: i64 },
    /// Pop the head (the track that just got played). Reply: QueueState.
    QueuePop { device_url: String },
    /// Remove a specific queue entry by its id. Reply: QueueState.
    QueueRemove(i64),
    /// Clear the queue. Reply: QueueState.
    QueueClear { device_url: String },
    /// Load the current queue state. Reply: QueueState.
    LoadQueue { device_url: String },
    /// List all devices that have queued items + counts.
    /// Reply: QueueDevices.
    LoadQueueDevices,

    // ── Soundscape loop processor ───────────────────────────────────
    /// Process a stereo WAV file: run the random-walk loop finder and
    /// write the result as an OggOpus file at `output_path`. Emits
    /// SoundscapeProgress while running and SoundscapeDone (or
    /// SoundscapeFailed) on completion.
    ProcessSoundscape {
        input_path: String,
        output_path: String,
        target_seconds: f32,
        tolerance_seconds: f32,
        n_candidates: usize,
        bitrate_kbps: u32,
        fade_ms: u32,
    },

    // ── Stream mixer offline processor ─────────────────────────────
    /// Process a stereo WAV file through the realtime mixer with the
    /// supplied parameters and write the result as OggOpus. Uses the
    /// exact same `RealtimeMixer::process_buffer` that an eventual
    /// audio thread would call, so this doubles as a correctness
    /// preview for live processing.
    ProcessThroughMixer {
        input_path: String,
        output_path: String,
        bitrate_kbps: u32,
        params: crate::realtime_mixer::MixerParams,
    },
    /// Compute the average power spectrum of a WAV. Reply:
    /// SpectrumDone with JSON [{hz, db}, ...] aligned to the same
    /// log-spaced axis the mixer curve uses, so the QML side can
    /// draw both on top of each other.
    AnalyzeSpectrum { input_path: String },

    // ── Tags ─────────────────────────────────────────────────────────
    /// Create or update a tag with optional color. Reply: TagsList.
    UpsertTag { name: String, color: Option<String> },
    /// Attach a tag to a track. Reply: TrackTags.
    AttachTag { track_id: i64, tag_id: i64 },
    /// Detach a tag from a track. Reply: TrackTags.
    DetachTag { track_id: i64, tag_id: i64 },
    /// List all tags with counts. Reply: TagsList.
    ListAllTags,
    /// List tags attached to a specific track. Reply: TrackTags.
    LoadTrackTags(i64),
    /// Load all tracks with a given tag, ordered by play count.
    /// Reply: TrackList with context "tag:<id>".
    LoadTracksByTag(i64),

    // ── History / Artist aggregation ─────────────────────────────────
    /// Record a play (also bumps tracks.play_count). Reply: none (fire & forget).
    RecordPlayHistory { track_id: i64, device_url: String, duration_seconds: Option<i64> },
    /// Load recent play history. Reply: HistoryList.
    LoadRecentHistory(usize),
    /// Aggregate plays per hour-of-day across the last N days.
    /// Reply: HistoryStats (24 entries, [{hour, count}, ...]).
    LoadHistoryStats { lookback_days: i64 },
    /// Rolling KPI summary (total plays, top track, top artist).
    /// Reply: HistorySummary (single object).
    LoadHistorySummary { lookback_days: i64 },
    /// Top artists by total play count. Reply: ArtistsList.
    LoadTopArtists(usize),
    /// All artists alphabetical. Reply: ArtistsList.
    LoadAllArtistsAlpha,
    /// Top albums by total play count. Reply: AlbumsList.
    LoadTopAlbums(usize),
    /// All albums alphabetical by title. Reply: AlbumsList.
    LoadAllAlbumsAlpha { sort: String },
    /// Tracks belonging to a specific album. Reply: TrackList tagged
    /// "album:<title>:<primary_artist>".
    LoadTracksInAlbum { title: String, primary_artist: String },
    /// Top individual tracks by play count. Reply: TrackList tagged
    /// "smart:top".
    LoadTopTracks(usize),
    /// Tracks recently added to the library (sorted by file mtime
    /// desc). Reply: TrackList tagged "smart:recent".
    LoadRecentlyAdded(usize),
    /// Tracks the user has never played. Reply: TrackList tagged
    /// "smart:never".
    LoadNeverPlayed(usize),
    /// Advanced multi-term search. Reply: TrackList tagged "search:<query>".
    SearchAdvanced(String),
}

pub fn run(
    rx: Receiver<MediaCmd>,
    tx: Sender<ResultMessage>,
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    extra_known_artists: Vec<String>,
) {
    let mut conn = match catalog::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(ResultMessage::Error(format!("catalog open failed: {}", e)));
            return;
        }
    };

    let cancel = Arc::new(AtomicBool::new(false));

    // Immediately publish whatever's already in the catalog so the UI
    // populates instantly, even before any scan runs.
    publish_initial(&conn, &tx);

    while let Ok(cmd) = rx.recv() {
        match cmd {
            MediaCmd::RescanLibrary => {
                cancel.store(false, Ordering::Relaxed);
                if let Err(e) = do_walk_and_parse(&mut conn, &roots, &extra_known_artists, &tx) {
                    let _ = tx.send(ResultMessage::Error(format!("scan failed: {}", e)));
                    continue;
                }
                if let Err(e) = scanner::analyze_pending(&mut conn, &tx, &cancel) {
                    let _ = tx.send(ResultMessage::Error(format!("analyze failed: {}", e)));
                }
                publish_stats(&conn, &tx);
            }
            MediaCmd::QuickScan => {
                if let Err(e) = do_walk_and_parse(&mut conn, &roots, &extra_known_artists, &tx) {
                    let _ = tx.send(ResultMessage::Error(format!("quick scan failed: {}", e)));
                }
                publish_stats(&conn, &tx);
            }
            MediaCmd::ScanWithRoots(user_roots) => {
                let chosen: Vec<PathBuf> = if user_roots.is_empty() {
                    roots.clone()
                } else {
                    user_roots.iter().map(PathBuf::from).collect()
                };
                if let Err(e) = do_walk_and_parse(&mut conn, &chosen, &extra_known_artists, &tx) {
                    let _ = tx.send(ResultMessage::Error(format!("scan failed: {}", e)));
                }
                publish_stats(&conn, &tx);
            }
            MediaCmd::AnalyzeBacklog => {
                cancel.store(false, Ordering::Relaxed);
                if let Err(e) = scanner::analyze_pending(&mut conn, &tx, &cancel) {
                    let _ = tx.send(ResultMessage::Error(format!("analyze failed: {}", e)));
                }
                publish_stats(&conn, &tx);
            }
            MediaCmd::CancelAnalysis => {
                cancel.store(true, Ordering::Relaxed);
            }

            MediaCmd::LoadHome => {
                publish_stats(&conn, &tx);
                publish_resume(&conn, &tx);
                publish_recent(&conn, &tx);
                publish_bpm_bands(&conn, &tx);
            }
            MediaCmd::LoadResume    => publish_resume(&conn, &tx),
            MediaCmd::LoadRecent    => publish_recent(&conn, &tx),
            MediaCmd::LoadArtists   => publish_artists(&conn, &tx),
            MediaCmd::LoadTags      => publish_tags(&conn, &tx),
            MediaCmd::LoadBpmBandCounts => publish_bpm_bands(&conn, &tx),

            // Paginated counterparts. Each returns a Page variant
            // carrying (offset, total) so the QML side can
            // size its model. The QML pattern:
            //   - Initial load: request offset=0, limit=50
            //   - On near-bottom scroll: request offset=current_len,
            //     limit=50 — concatenate into the existing model
            //   - On context change (different artist, sort flip):
            //     drop existing model, request offset=0 again
            MediaCmd::LoadArtistsPage { offset, limit, sort } => {
                let sort = catalog::ArtistPageSort::from_tag(&sort);
                match catalog::artists_with_counts_page_sorted(&conn, offset, limit, sort) {
                    Ok((page, total)) => {
                        let _ = tx.send(ResultMessage::Page {
                            context: "artists".into(),
                            json: serde_json::to_string(&page).unwrap_or_default(),
                            offset, total,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(format!(
                            "LoadArtistsPage failed: {}", e)));
                    }
                }
            }
            MediaCmd::LoadRecentPage { offset, limit } => {
                match catalog::recent_page(&conn, offset, limit) {
                    Ok((page, total)) => {
                        let _ = tx.send(ResultMessage::Page {
                            context: "recent".into(),
                            json: serde_json::to_string(&page).unwrap_or_default(),
                            offset, total,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(format!(
                            "LoadRecentPage failed: {}", e)));
                    }
                }
            }
            MediaCmd::LoadByArtistPage { artist_id, offset, limit } => {
                match catalog::by_artist_id_page(&conn, artist_id, offset, limit) {
                    Ok((page, total)) => {
                        let _ = tx.send(ResultMessage::Page {
                            context: format!("artist:{}", artist_id),
                            json: serde_json::to_string(&page).unwrap_or_default(),
                            offset, total,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(format!(
                            "LoadByArtistPage failed: {}", e)));
                    }
                }
            }

            MediaCmd::LoadAllTracksPage { offset, limit, sort } => {
                let sort = catalog::TrackSort::from_tag(&sort);
                match catalog::all_tracks_page_sorted(&conn, offset, limit, sort) {
                    Ok((page, total)) => {
                        let _ = tx.send(ResultMessage::Page {
                            context: "all_tracks".to_string(),
                            json: serde_json::to_string(&page).unwrap_or_default(),
                            offset, total,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(format!(
                            "LoadAllTracksPage failed: {}", e)));
                    }
                }
            }

            MediaCmd::LoadByArtist(id) => {
                if let Ok(list) = catalog::by_artist_id(&conn, id) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: format!("artist:{}", id),
                        json: serde_json::to_string(&list).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::LoadByTag(id) => {
                if let Ok(list) = catalog::by_tag_id(&conn, id) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: format!("tag:{}", id),
                        json: serde_json::to_string(&list).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::LoadByBpmBand(lo, hi) => {
                if let Ok(list) = catalog::by_bpm_range(&conn, lo, hi) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: format!("bpm:{}-{}", lo as i32, hi as i32),
                        json: serde_json::to_string(&list).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::LoadByCamelot(c) => {
                if let Ok(list) = catalog::by_camelot(&conn, &c) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: format!("camelot:{}", c),
                        json: serde_json::to_string(&list).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::Search(query) => {
                if let Ok(list) = catalog::search(&conn, &query, 200) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: format!("search:{}", query),
                        json: serde_json::to_string(&list).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::LoadCompatibleNow { bpm, camelot } => {
                if let Ok(list) = smart_cards::compatible_with(
                    &conn, bpm, camelot.as_deref(), 50)
                {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: "compatible".to_string(),
                        json: serde_json::to_string(&list).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::RecordPlay(id) => {
                let _ = smart_cards::record_play(&conn, id);
            }
            MediaCmd::LoadKeyBpmForTrack(id) => {
                let row: Option<(Option<f32>, Option<String>, Option<String>)> = conn.query_row(
                    "SELECT bpm, camelot, key_text FROM tracks WHERE id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                ).ok();
                if let Some((bpm, camelot, key_text)) = row {
                    let _ = tx.send(ResultMessage::NowPlayingKeyBpm { bpm, camelot, key_text });
                }
            }
            MediaCmd::SaveAnticipatorState(json) => {
                if let Err(e) = catalog::set_app_state(&conn, "anticipator_counts", &json) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("save anticipator: {}", e)));
                }
            }
            MediaCmd::LoadAnticipatorState => {
                let json = catalog::get_app_state(&conn, "anticipator_counts")
                    .ok().flatten().unwrap_or_default();
                let _ = tx.send(ResultMessage::AnticipatorStateLoaded(json));
            }
            MediaCmd::SaveAppKv { key, value } => {
                if let Err(e) = catalog::set_app_state(&conn, &key, &value) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("save app_kv[{}]: {}", key, e)));
                }
            }
            MediaCmd::LoadAppKv { key } => {
                let value = catalog::get_app_state(&conn, &key)
                    .ok().flatten().unwrap_or_default();
                let _ = tx.send(ResultMessage::AppKvLoaded { key, value });
            }
            MediaCmd::ResolveForPlay(id) => {
                // Pull path + display title/artist. The tracks table
                // stores the parsed title in `title`; an explicit user
                // override (track_overrides) can supersede it.
                let row: Option<(String, Option<String>, Option<String>)> = conn.query_row(
                    "SELECT t.path, t.title,
                            (SELECT a.name FROM artists a
                             JOIN track_artists ta ON ta.artist_id = a.id
                             WHERE ta.track_id = t.id AND ta.role = 'primary'
                             ORDER BY ta.ord ASC LIMIT 1) AS primary_artist
                     FROM tracks t WHERE t.id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                ).ok();
                // Apply override on top if present.
                let ov = catalog::get_override(&conn, id).ok().flatten();
                if let Some((path, parsed_title, parsed_artist)) = row {
                    let title = ov.as_ref().and_then(|o| o.title.clone())
                        .or(parsed_title)
                        .unwrap_or_else(|| {
                            std::path::Path::new(&path)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Track")
                                .to_string()
                        });
                    let artist = ov.as_ref().and_then(|o| o.artist.clone())
                        .or(parsed_artist)
                        .unwrap_or_else(|| "Unknown".to_string());
                    // Read duration from the file's audio properties so
                    // the playbar progress fill is accurate. lofty reads only
                    // the container header (a few ms); not a full decode.
                    // Some Opus/streamed files don't expose duration in the
                    // header lofty parses, so it returns 0 (the reported
                    // "seek_to dur=0" / no progress-fill case). When that
                    // happens, fall back to ffprobe (ships with the ffmpeg
                    // we already require for decode), which reads the real
                    // duration. Still 0 only if both fail.
                    let mut duration_secs = lofty::read_from_path(&path)
                        .ok()
                        .map(|f| f.properties().duration().as_secs() as u32)
                        .unwrap_or(0);
                    if duration_secs == 0 {
                        duration_secs = ffprobe_duration_secs(&path);
                    }
                    let _ = tx.send(ResultMessage::TrackForPlay {
                        track_id: id, path, title, artist, duration_secs,
                    });
                } else {
                    let _ = tx.send(ResultMessage::Error(
                        format!("resolve_for_play: track {} not found", id)));
                }
            }
            MediaCmd::SetOverride { track_id, title, artist, album, year } => {
                let ov = catalog::TrackOverride { track_id, title, artist, album, year };
                if let Err(e) = catalog::set_override(&conn, &ov) {
                    let _ = tx.send(ResultMessage::Error(format!("set_override: {}", e)));
                } else {
                    // Re-publish recent so the UI updates immediately with the
                    // edited fields. Cheap, ~50 rows.
                    publish_recent(&conn, &tx);
                }
            }

            // ── Playlist commands ──
            MediaCmd::CreatePlaylist { name, description } => {
                match library::create_playlist(&conn, &name, &description) {
                    Ok(id) => {
                        let _ = tx.send(ResultMessage::PlaylistCreated { id, name });
                        publish_playlists(&conn, &tx);
                    }
                    Err(e) => { let _ = tx.send(ResultMessage::Error(
                        format!("create_playlist: {}", e))); }
                }
            }
            MediaCmd::DeletePlaylist(id) => {
                if let Err(e) = library::delete_playlist(&conn, id) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("delete_playlist: {}", e)));
                }
                publish_playlists(&conn, &tx);
            }
            MediaCmd::UpdatePlaylistMeta { id, name, description } => {
                let n = if name.is_empty() { None } else { Some(name.as_str()) };
                let d = if description.is_empty() { None } else { Some(description.as_str()) };
                if let Err(e) = library::update_playlist_meta(&conn, id, n, d) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("update_playlist_meta: {}", e)));
                }
                publish_playlists(&conn, &tx);
            }
            MediaCmd::AddTrackToPlaylist { playlist_id, track_id } => {
                if let Err(e) = library::add_to_playlist(&conn, playlist_id, track_id) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("add_to_playlist: {}", e)));
                }
                publish_playlist_tracks(&conn, &tx, playlist_id);
                publish_playlists(&conn, &tx);
            }
            MediaCmd::AddManyToPlaylist { playlist_id, track_ids } => {
                match library::add_many_to_playlist(&mut conn, playlist_id, &track_ids) {
                    Ok(n) => { eprintln!("playlist {}: added {} tracks", playlist_id, n); }
                    Err(e) => { let _ = tx.send(ResultMessage::Error(
                        format!("add_many_to_playlist: {}", e))); }
                }
                publish_playlist_tracks(&conn, &tx, playlist_id);
                publish_playlists(&conn, &tx);
            }
            MediaCmd::RemoveTrackFromPlaylist { playlist_id, track_id } => {
                if let Err(e) = library::remove_from_playlist(&mut conn, playlist_id, track_id) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("remove_from_playlist: {}", e)));
                }
                publish_playlist_tracks(&conn, &tx, playlist_id);
                publish_playlists(&conn, &tx);
            }
            MediaCmd::ReorderPlaylist { playlist_id, track_id, new_ord } => {
                if let Err(e) = library::reorder_playlist(&mut conn, playlist_id, track_id, new_ord) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("reorder_playlist: {}", e)));
                }
                publish_playlist_tracks(&conn, &tx, playlist_id);
            }
            MediaCmd::SetTrackRating { track_id, rating } => {
                if let Err(e) = catalog::set_track_rating(&mut conn, track_id, rating) {
                    let _ = tx.send(ResultMessage::Error(
                        format!("set_track_rating: {}", e)));
                } else {
                    // Echo the confirmed rating back so the UI can update
                    // optimistically-then-confirmed.
                    let r = catalog::track_rating(&conn, track_id).unwrap_or(0);
                    let _ = tx.send(ResultMessage::TrackRating { track_id, rating: r });
                }
            }
            MediaCmd::WriteTagsToFile { track_id, title, artist, album, year } => {
                // Resolve the file path, then write the tags off the UI
                // thread. Report success/failure via a status message.
                let path: Option<String> = conn.query_row(
                    "SELECT path FROM tracks WHERE id = ?1",
                    rusqlite::params![track_id], |r| r.get(0)).ok();
                match path {
                    Some(p) => {
                        let edit = tag_writer::TagEdit { title, artist, album, year };
                        match tag_writer::write_tags(std::path::Path::new(&p), &edit) {
                            Ok(()) => {
                                let _ = tx.send(ResultMessage::TagWriteResult {
                                    track_id, ok: true,
                                    message: "Saved tags to file".to_string() });
                            }
                            Err(e) => {
                                let _ = tx.send(ResultMessage::TagWriteResult {
                                    track_id, ok: false,
                                    message: format!("Couldn't write tags: {}", e) });
                            }
                        }
                    }
                    None => {
                        let _ = tx.send(ResultMessage::TagWriteResult {
                            track_id, ok: false,
                            message: "Track has no file path".to_string() });
                    }
                }
            }
            MediaCmd::ListPlaylists => {
                publish_playlists(&conn, &tx);
            }
            MediaCmd::LoadPlaylistTracks(id) => {
                publish_playlist_tracks(&conn, &tx, id);
            }

            // ── Queue commands ──
            MediaCmd::QueuePush { track_id, device_url } => {
                if let Err(e) = library::queue_push(&conn, track_id, &device_url) {
                    let _ = tx.send(ResultMessage::Error(format!("queue_push: {}", e)));
                }
                publish_queue(&conn, &tx, &device_url);
            }
            MediaCmd::QueueInsert { track_id, device_url, at } => {
                if let Err(e) = library::queue_insert(&mut conn, track_id, &device_url, at) {
                    let _ = tx.send(ResultMessage::Error(format!("queue_insert: {}", e)));
                }
                publish_queue(&conn, &tx, &device_url);
            }
            MediaCmd::QueuePop { device_url } => {
                if let Err(e) = library::queue_pop(&mut conn, &device_url) {
                    let _ = tx.send(ResultMessage::Error(format!("queue_pop: {}", e)));
                }
                publish_queue(&conn, &tx, &device_url);
            }
            MediaCmd::QueueRemove(qid) => {
                // We don't know device_url from qid alone; the row delete
                // is independent of device_url. After the remove we publish
                // BOTH the global queue and any per-device queues that
                // might have been affected. Simple version: only publish
                // the global queue (the common case) — per-device callers
                // refresh from QML on the next tick.
                if let Err(e) = library::queue_remove(&mut conn, qid) {
                    let _ = tx.send(ResultMessage::Error(format!("queue_remove: {}", e)));
                }
                publish_queue(&conn, &tx, "");
            }
            MediaCmd::QueueClear { device_url } => {
                if let Err(e) = library::queue_clear(&conn, &device_url) {
                    let _ = tx.send(ResultMessage::Error(format!("queue_clear: {}", e)));
                }
                publish_queue(&conn, &tx, &device_url);
            }
            MediaCmd::LoadQueue { device_url } => {
                publish_queue(&conn, &tx, &device_url);
            }
            MediaCmd::LoadQueueDevices => {
                match library::queue_devices(&conn) {
                    Ok(list) => {
                        let json = serde_json::to_string(&list).unwrap_or_default();
                        let _ = tx.send(ResultMessage::QueueDevices(json));
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(
                            format!("queue_devices: {}", e)));
                    }
                }
            }
            MediaCmd::ProcessSoundscape { input_path, output_path,
                target_seconds, tolerance_seconds, n_candidates,
                bitrate_kbps, fade_ms } =>
            {
                // Run synchronously on the media worker thread — the
                // search is CPU-bound and short (a few seconds for a
                // 30s input at default settings). Streaming progress
                // updates back via the result channel keeps the QML
                // progress bar live.
                let tx_progress = tx.clone();
                let result = (|| -> Result<crate::looper::LoopResult, String> {
                    let path = std::path::PathBuf::from(&input_path);
                    eprintln!("looper: reading audio {}", path.display());
                    let buf = crate::looper::read_audio(&path)?;
                    eprintln!("looper: decoded {} samples @ {} Hz ({:.1}s)",
                              buf.n_samples(), buf.sample_rate, buf.duration_seconds());
                    let params = crate::looper::LoopParams {
                        target_seconds,
                        tolerance_seconds,
                        n_candidates,
                        n_refine: 16,
                        n_refine_steps: 300,
                        w_pop: 1.0,
                        w_mfcc: 1.0,
                        w_length: 0.3,
                        // Seed varies per run so users see different
                        // candidates on re-runs of the same input;
                        // hash input_path + now-seconds for a stable
                        // PRNG seed inside one invocation.
                        seed: {
                            use std::collections::hash_map::DefaultHasher;
                            use std::hash::{Hash, Hasher};
                            let mut h = DefaultHasher::new();
                            input_path.hash(&mut h);
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                                .hash(&mut h);
                            h.finish()
                        },
                    };
                    let result = crate::looper::find_best_loop(&buf, &params, |p| {
                        // Throttle progress updates to ~20Hz worth so
                        // the channel doesn't get flooded.
                        let _ = tx_progress.send(ResultMessage::SoundscapeProgress(p));
                    });
                    eprintln!("looper: best loop start={} end={} score={:.4} \
                               pop={:.4} mfcc={:.4} length={:.2}s",
                               result.start, result.end, result.score,
                               result.pop_score, result.mfcc_score, result.length_seconds);

                    // Write OggOpus output. Ensure the parent directory
                    // exists first — when the user picks the default
                    // `~/Music/Soundscapes/<name>.opus` path on a fresh
                    // setup, the Soundscapes folder might not exist
                    // yet. `create_dir_all` is idempotent: no-op if
                    // already there, creates the chain if not.
                    let fade_samples = ((fade_ms as f32 / 1000.0)
                                       * buf.sample_rate as f32) as usize;
                    let out = std::path::PathBuf::from(&output_path);
                    if let Some(parent) = out.parent() {
                        if !parent.as_os_str().is_empty() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                return Err(format!(
                                    "couldn't create output directory {}: {}",
                                    parent.display(), e
                                ).into());
                            }
                        }
                    }
                    crate::looper::write_loop_opus(&buf, &result, fade_samples,
                                                   bitrate_kbps, &out)?;
                    eprintln!("looper: wrote {} ({} bytes)",
                              out.display(),
                              std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0));
                    Ok(result)
                })();
                match result {
                    Ok(lr) => {
                        let size_bytes = std::fs::metadata(&output_path)
                            .map(|m| m.len()).unwrap_or(0);
                        let json = serde_json::json!({
                            "output_path": output_path,
                            "size_bytes":  size_bytes,
                            "loop":        lr,
                        });
                        let _ = tx.send(ResultMessage::SoundscapeDone(
                            json.to_string()));
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::SoundscapeFailed(e));
                    }
                }
            }
            MediaCmd::ProcessThroughMixer { input_path, output_path,
                bitrate_kbps, params } =>
            {
                // Synchronous on the media worker thread. The mixer
                // is fast — a 5-minute stereo file at 44.1 kHz
                // processes in well under a second on modern CPUs.
                // Progress callbacks stream back to QML for the bar.
                let tx_progress = tx.clone();
                let result = crate::realtime_mixer::process_file_through_mixer(
                    std::path::Path::new(&input_path),
                    std::path::Path::new(&output_path),
                    params,
                    bitrate_kbps,
                    |p| {
                        let _ = tx_progress.send(
                            ResultMessage::MixerProcessProgress(p));
                    },
                );
                match result {
                    Ok(frames) => {
                        let size_bytes = std::fs::metadata(&output_path)
                            .map(|m| m.len()).unwrap_or(0);
                        let json = serde_json::json!({
                            "output_path": output_path,
                            "size_bytes":  size_bytes,
                            "frames":      frames,
                        });
                        let _ = tx.send(ResultMessage::MixerProcessDone(
                            json.to_string()));
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::MixerProcessFailed(e));
                    }
                }
            }
            MediaCmd::AnalyzeSpectrum { input_path } => {
                // 64 points to match RealtimeMixer::compute_curves so
                // the canvas can overlay them directly. 200 max
                // frames is plenty for a stable average (≈ 4.6s of
                // material at 44.1 kHz before striding).
                let result = crate::looper::spectrum_average(
                    std::path::Path::new(&input_path),
                    64, 20.0, 20_000.0, 200,
                );
                match result {
                    Ok(points) => {
                        let pts: Vec<_> = points.iter().map(|(hz, db)| {
                            serde_json::json!({ "hz": hz, "db": db })
                        }).collect();
                        let json = serde_json::json!({
                            "input_path": input_path,
                            "points": pts,
                        }).to_string();
                        let _ = tx.send(ResultMessage::SpectrumDone(json));
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::SpectrumFailed(e));
                    }
                }
            }

            // ── Tags ──
            MediaCmd::UpsertTag { name, color } => {
                if let Err(e) = library::upsert_tag(&conn, &name, color.as_deref()) {
                    let _ = tx.send(ResultMessage::Error(format!("upsert_tag: {}", e)));
                }
                publish_tags_list(&conn, &tx);
            }
            MediaCmd::AttachTag { track_id, tag_id } => {
                if let Err(e) = library::attach_tag(&conn, track_id, tag_id) {
                    let _ = tx.send(ResultMessage::Error(format!("attach_tag: {}", e)));
                }
                publish_track_tags(&conn, &tx, track_id);
                publish_tags_list(&conn, &tx);
            }
            MediaCmd::DetachTag { track_id, tag_id } => {
                if let Err(e) = library::detach_tag(&conn, track_id, tag_id) {
                    let _ = tx.send(ResultMessage::Error(format!("detach_tag: {}", e)));
                }
                publish_track_tags(&conn, &tx, track_id);
                publish_tags_list(&conn, &tx);
            }
            MediaCmd::ListAllTags => publish_tags_list(&conn, &tx),
            MediaCmd::LoadTrackTags(tid) => publish_track_tags(&conn, &tx, tid),
            MediaCmd::LoadTracksByTag(tag_id) => {
                match library::tracks_by_tag(&conn, tag_id, 500) {
                    Ok(list) => {
                        if list.is_empty() {
                            // Diagnostic: when a user-visible tag
                            // returns zero tracks, surface enough
                            // context to debug (tag name + total
                            // track_tags row count for this id).
                            // The user reported "tags page still
                            // pulls empty tracklists for known
                            // good tags" — this line tells us
                            // whether track_tags has rows for the
                            // requested tag id at all, vs the
                            // tracks_by_tag query mis-joining.
                            let tag_name: String = conn.query_row(
                                "SELECT name FROM tags WHERE id=?1",
                                rusqlite::params![tag_id],
                                |r| r.get(0)
                            ).unwrap_or_else(|_| "<missing>".to_string());
                            let row_count: i64 = conn.query_row(
                                "SELECT COUNT(*) FROM track_tags WHERE tag_id=?1",
                                rusqlite::params![tag_id],
                                |r| r.get(0)
                            ).unwrap_or(0);
                            eprintln!(
                                "tracks_by_tag: tag_id={} ('{}') → 0 tracks. \
                                 track_tags row count for this tag: {}. \
                                 If 0, this tag has no attachments; \
                                 if >0, the join is dropping rows (check \
                                 tracks table integrity).",
                                tag_id, tag_name, row_count);
                        }
                        let json = serde_json::to_string(&list).unwrap_or_default();
                        let _ = tx.send(ResultMessage::TrackList {
                            context: format!("tag:{}", tag_id),
                            json,
                        });
                    }
                    Err(e) => { let _ = tx.send(ResultMessage::Error(
                        format!("tracks_by_tag: {}", e))); }
                }
            }

            // ── History / artists / search ──
            MediaCmd::RecordPlayHistory { track_id, device_url, duration_seconds } => {
                if let Err(e) = library::record_history(&conn, track_id, &device_url, duration_seconds) {
                    let _ = tx.send(ResultMessage::Error(format!("record_history: {}", e)));
                }
            }
            MediaCmd::LoadRecentHistory(limit) => {
                publish_history(&conn, &tx, limit);
            }
            MediaCmd::LoadHistoryStats { lookback_days } => {
                match library::plays_by_hour_of_day(&conn, lookback_days) {
                    Ok(stats) => {
                        let arr: Vec<_> = stats.iter().map(|(h, n)| {
                            serde_json::json!({ "hour": h, "count": n })
                        }).collect();
                        let json = serde_json::to_string(&arr).unwrap_or_default();
                        let _ = tx.send(ResultMessage::HistoryStats(json));
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(
                            format!("plays_by_hour_of_day: {}", e)));
                    }
                }
            }
            MediaCmd::LoadHistorySummary { lookback_days } => {
                match library::history_summary(&conn, lookback_days) {
                    Ok(summary) => {
                        let json = serde_json::to_string(&summary).unwrap_or_default();
                        let _ = tx.send(ResultMessage::HistorySummary(json));
                    }
                    Err(e) => {
                        let _ = tx.send(ResultMessage::Error(
                            format!("history_summary: {}", e)));
                    }
                }
            }
            MediaCmd::LoadTopArtists(limit) => {
                publish_top_artists(&conn, &tx, limit);
            }
            MediaCmd::LoadAllArtistsAlpha => {
                publish_all_artists_alpha(&conn, &tx);
            }
            MediaCmd::LoadTopAlbums(limit) => {
                publish_top_albums(&conn, &tx, limit);
            }
            MediaCmd::LoadAllAlbumsAlpha { sort } => {
                publish_all_albums_alpha(&conn, &tx, &sort);
            }
            MediaCmd::LoadTracksInAlbum { title, primary_artist } => {
                // Track list for a specific (title, primary_artist) pair.
                // Tagged with "album:..." so the result handler in main.rs
                // can distinguish album tracks from search results or
                // playlist tracks (which share TrackList shape).
                if let Ok(tracks) = library::tracks_in_album(&conn, &title, &primary_artist) {
                    let json = serde_json::to_string(&tracks).unwrap_or_default();
                    let _ = tx.send(ResultMessage::TrackList {
                        context: format!("album:{}:{}", title, primary_artist),
                        json,
                    });
                }
            }
            MediaCmd::LoadTopTracks(limit) => {
                if let Ok(tracks) = library::top_tracks_by_plays(&conn, limit) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: "smart:top".to_string(),
                        json: serde_json::to_string(&tracks).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::LoadRecentlyAdded(limit) => {
                if let Ok(tracks) = library::recently_added(&conn, limit) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: "smart:recent".to_string(),
                        json: serde_json::to_string(&tracks).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::LoadNeverPlayed(limit) => {
                if let Ok(tracks) = library::never_played(&conn, limit) {
                    let _ = tx.send(ResultMessage::TrackList {
                        context: "smart:never".to_string(),
                        json: serde_json::to_string(&tracks).unwrap_or_default(),
                    });
                }
            }
            MediaCmd::SearchAdvanced(query) => {
                publish_search(&conn, &tx, &query);
            }
        }
    }
}

fn do_walk_and_parse(
    conn: &mut rusqlite::Connection,
    roots: &[PathBuf],
    extra: &[String],
    tx: &Sender<ResultMessage>,
) -> rusqlite::Result<()> {
    let pending = scanner::walk_and_upsert(conn, roots, tx)?;
    scanner::parse_pending(conn, &pending, extra, tx)?;
    // Re-link durable path-keyed tag attachments onto the (possibly new)
    // track ids. If a rescan re-inserted tracks with fresh ids, their
    // id-keyed tag rows cascaded away — this replays the path-keyed
    // mirror so the user's manually-applied tags reappear.
    match library::relink_tags_by_path(conn) {
        Ok(n) if n > 0 => eprintln!(
            "scanner: re-linked {} tag attachment(s) from path-keyed store \
             (tags survive rescans now)", n),
        Ok(_) => {}
        Err(e) => eprintln!("scanner: tag relink failed: {}", e),
    }
    // Same idea for star ratings: replay the path-keyed rating mirror onto
    // any tracks that came back with a fresh id and a zeroed rating.
    match catalog::relink_ratings_by_path(conn) {
        Ok(n) if n > 0 => eprintln!(
            "scanner: re-linked {} track rating(s) from path-keyed store", n),
        Ok(_) => {}
        Err(e) => eprintln!("scanner: rating relink failed: {}", e),
    }
    // ── Publish derived lists ──
    //
    // The Library page reads from TrackList messages (context
    // "recent", "by_artist", "by_tag", etc) — these get published
    // once at worker startup via publish_initial and then sit
    // stale. After a scan that actually wrote new metadata, the
    // UI is showing the *previous* snapshot until something else
    // triggers a refresh; in practice users see empty rows or
    // outdated counts until they navigate away + back.
    //
    // Re-publishing stats + recent + artists + tags + bpm_bands
    // costs the same as the initial publish (each one is a single
    // small SELECT against indexed columns) and matches what
    // publish_initial does at startup. The QML side already binds
    // to these channels for live updates; it just wasn't getting
    // any after the first scan completed.
    publish_stats(conn, tx);
    publish_recent(conn, tx);
    publish_artists(conn, tx);
    publish_tags(conn, tx);
    publish_bpm_bands(conn, tx);
    Ok(())
}

fn publish_initial(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    publish_stats(conn, tx);
    publish_resume(conn, tx);
    publish_recent(conn, tx);
    publish_bpm_bands(conn, tx);
}

fn publish_stats(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(s) = catalog::stats(conn) {
        let _ = tx.send(ResultMessage::CatalogStats(s));
    }
}

fn publish_resume(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(r) = smart_cards::resume_state(conn) {
        let _ = tx.send(ResultMessage::ResumeState(r));
    }
}

fn publish_recent(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(list) = catalog::recent(conn, 50) {
        let _ = tx.send(ResultMessage::TrackList {
            context: "recent".to_string(),
            json: serde_json::to_string(&list).unwrap_or_default(),
        });
    }
}

fn publish_artists(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(list) = catalog::artists_with_counts(conn) {
        let _ = tx.send(ResultMessage::GroupList {
            context: "artists".to_string(),
            json: serde_json::to_string(&list).unwrap_or_default(),
        });
    }
}

fn publish_tags(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(list) = catalog::tags_with_counts(conn) {
        let _ = tx.send(ResultMessage::GroupList {
            context: "tags".to_string(),
            json: serde_json::to_string(&list).unwrap_or_default(),
        });
    }
}

fn publish_bpm_bands(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(counts) = smart_cards::count_per_band(conn) {
        let payload: Vec<serde_json::Value> = counts.into_iter().map(|(b, n)| {
            serde_json::json!({
                "label": b.label, "lo": b.lo, "hi": b.hi, "count": n,
            })
        }).collect();
        let _ = tx.send(ResultMessage::BpmBandCounts(
            serde_json::to_string(&payload).unwrap_or_default()
        ));
    }
}

// ─── New publishers: playlists, queue, tags-list, history, artists-detail ───

fn playlists_to_json(list: &[library::PlaylistSummary]) -> String {
    let arr: Vec<serde_json::Value> = list.iter().map(|p| serde_json::json!({
        "id": p.id, "name": p.name, "description": p.description,
        "track_count": p.track_count, "created_at": p.created_at,
        "updated_at": p.updated_at, "cover_track_id": p.cover_track_id,
    })).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn publish_playlists(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(list) = library::list_playlists(conn) {
        let _ = tx.send(ResultMessage::PlaylistsList(playlists_to_json(&list)));
    }
}

fn publish_playlist_tracks(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, id: i64) {
    if let Ok(list) = library::tracks_in_playlist(conn, id) {
        // TrackSummary is Serialize.
        let json = serde_json::to_string(&list).unwrap_or_default();
        let _ = tx.send(ResultMessage::PlaylistTracks { playlist_id: id, json });
    }
}

fn queue_to_json(entries: &[library::QueueEntry]) -> String {
    let arr: Vec<serde_json::Value> = entries.iter().map(|e| serde_json::json!({
        "queue_id": e.queue_id, "track_id": e.track_id, "ord": e.ord,
        "title": e.title, "primary_artist": e.primary_artist, "bpm": e.bpm,
    })).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn publish_queue(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, device_url: &str) {
    if let Ok(list) = library::queue_list(conn, device_url) {
        let _ = tx.send(ResultMessage::QueueState {
            device_url: device_url.to_string(),
            json: queue_to_json(&list),
        });
    }
}

fn tags_to_json(tags: &[library::TagInfo]) -> String {
    let arr: Vec<serde_json::Value> = tags.iter().map(|t| serde_json::json!({
        "id": t.id, "name": t.name, "color": t.color, "track_count": t.track_count,
    })).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn publish_tags_list(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(list) = library::list_tags(conn) {
        let _ = tx.send(ResultMessage::TagsList(tags_to_json(&list)));
    }
}

fn publish_track_tags(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, track_id: i64) {
    if let Ok(list) = library::track_tags(conn, track_id) {
        let _ = tx.send(ResultMessage::TrackTags {
            track_id,
            json: tags_to_json(&list),
        });
    }
}

fn history_to_json(items: &[library::HistoryEntry]) -> String {
    let arr: Vec<serde_json::Value> = items.iter().map(|h| serde_json::json!({
        "track_id": h.track_id, "played_at": h.played_at,
        "device_url": h.device_url, "title": h.title,
        "primary_artist": h.primary_artist,
    })).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn publish_history(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, limit: usize) {
    if let Ok(list) = library::recent_history(conn, limit) {
        let _ = tx.send(ResultMessage::HistoryList(history_to_json(&list)));
    }
}

fn artist_details_to_json(items: &[library::ArtistDetail]) -> String {
    let arr: Vec<serde_json::Value> = items.iter().map(|a| serde_json::json!({
        "id": a.id, "name": a.name, "track_count": a.track_count,
        "total_plays": a.total_plays, "last_played_at": a.last_played_at,
    })).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn publish_top_artists(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, limit: usize) {
    if let Ok(list) = library::top_artists_by_plays(conn, limit) {
        let _ = tx.send(ResultMessage::ArtistsList {
            context: "top".to_string(),
            json: artist_details_to_json(&list),
        });
    }
}

fn publish_all_artists_alpha(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>) {
    if let Ok(list) = library::all_artists_alpha(conn) {
        let _ = tx.send(ResultMessage::ArtistsList {
            context: "alpha".to_string(),
            json: artist_details_to_json(&list),
        });
    }
}

fn album_summaries_to_json(items: &[library::AlbumSummary]) -> String {
    let arr: Vec<serde_json::Value> = items.iter().map(|a| serde_json::json!({
        "title":          a.title,
        "primary_artist": a.primary_artist,
        "year":           a.year,
        "track_count":    a.track_count,
        "total_plays":    a.total_plays,
        "last_played_at": a.last_played_at,
    })).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn publish_top_albums(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, limit: usize) {
    if let Ok(mut list) = library::top_albums_by_plays(conn, limit) {
        // The "top" query filters to albums with total_plays > 0. On a
        // freshly-scanned library nothing has been played yet, so that
        // returns empty and the page reads "No albums in your library"
        // even though the catalog is full. Fall back to the alphabetical
        // list so the user always sees their albums; once plays
        // accumulate, the real top ordering takes over.
        if list.is_empty() {
            if let Ok(alpha) = library::all_albums_alpha(conn) {
                list = alpha;
            }
        }
        let _ = tx.send(ResultMessage::AlbumsList {
            context: "top".to_string(),
            json: album_summaries_to_json(&list),
        });
    }
}

fn publish_all_albums_alpha(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, sort: &str) {
    let sort = library::AlbumSort::from_tag(sort);
    if let Ok(list) = library::all_albums_sorted(conn, sort) {
        let _ = tx.send(ResultMessage::AlbumsList {
            context: "alpha".to_string(),
            json: album_summaries_to_json(&list),
        });
    }
}

fn publish_search(conn: &rusqlite::Connection, tx: &Sender<ResultMessage>, query: &str) {
    if let Ok(list) = library::search_advanced(conn, query, 200) {
        let json = serde_json::to_string(&list).unwrap_or_default();
        let _ = tx.send(ResultMessage::TrackList {
            context: format!("search:{}", query),
            json,
        });
    }
}
