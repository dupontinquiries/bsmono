// src/smart_cards.rs
//
// Pre-canned queries used by the home view cards. Each function returns
// a Vec<TrackSummary>; the QObject serializes that to JSON for QML.
//
// Card design philosophy: every card answers the question "what should
// I play right now given <state>?" with a useful filter. No ML, no
// algorithmic playlists — just clear, predictable filters on real
// catalog data. Surprise comes from your own library, not from
// inferred taste.

use super::catalog::{self, TrackSummary};
use super::key_detection;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

// ─── BPM bands ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BpmBand {
    pub label: &'static str,
    pub lo: f32,
    pub hi: f32,
}

pub const BPM_BANDS: &[BpmBand] = &[
    BpmBand { label: "Slow",    lo: 0.0,   hi: 90.0 },
    BpmBand { label: "Mid",     lo: 90.0,  hi: 110.0 },
    BpmBand { label: "House",   lo: 110.0, hi: 128.0 },
    BpmBand { label: "Techno",  lo: 128.0, hi: 140.0 },
    BpmBand { label: "Fast",    lo: 140.0, hi: 300.0 },
];

pub fn count_per_band(conn: &Connection) -> rusqlite::Result<Vec<(BpmBand, i64)>> {
    let mut out = Vec::new();
    for band in BPM_BANDS {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tracks WHERE bpm BETWEEN ?1 AND ?2",
            params![band.lo, band.hi],
            |r| r.get(0),
        )?;
        out.push((*band, n));
    }
    Ok(out)
}

// ─── Compatible now ────────────────────────────────────────────────────────
//
// Given a currently-playing track (or a manual seed), return tracks that
// would mix well: same Camelot or a neighbor, BPM within ±6.

pub fn compatible_with(
    conn: &Connection,
    seed_bpm: Option<f32>,
    seed_camelot: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<TrackSummary>> {
    // Camelot neighbors as a comma-quoted list. SQLite doesn't let us
    // bind a list to IN, so we build it once as a literal — these come
    // from a closed enum of 24 valid Camelot codes, so SQL-injection
    // safe.
    let neighbors: Vec<String> = seed_camelot
        .map(|c| key_detection::camelot_neighbors(c))
        .unwrap_or_default();
    let camelot_list = if neighbors.is_empty() {
        String::new()
    } else {
        let inner = neighbors.iter()
            .map(|n| format!("'{}'", n.replace('\'', "")))
            .collect::<Vec<_>>()
            .join(",");
        format!("({})", inner)
    };

    let (where_clause, bpm_lo, bpm_hi, order_by_bpm) = match (seed_bpm, seed_camelot) {
        (Some(b), Some(_)) => (
            format!("WHERE t.bpm BETWEEN ?1 AND ?2 AND t.camelot IN {}", camelot_list),
            b - 6.0, b + 6.0, b,
        ),
        (Some(b), None) => (
            "WHERE t.bpm BETWEEN ?1 AND ?2".to_string(),
            b - 6.0, b + 6.0, b,
        ),
        (None, Some(_)) => (
            format!("WHERE t.camelot IN {}", camelot_list),
            0.0, 999.0, 120.0,
        ),
        (None, None) => (
            String::new(),
            0.0, 999.0, 120.0,
        ),
    };

    let sql = format!(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        {where_clause}
        ORDER BY ABS(COALESCE(t.bpm, 0) - {order_by_bpm}) ASC
        LIMIT {limit_n}
    "#, where_clause = where_clause, order_by_bpm = order_by_bpm, limit_n = limit as i64);

    let mut stmt = conn.prepare(&sql)?;

    let mut out = Vec::new();
    let row_map = |r: &rusqlite::Row| Ok(TrackSummary {
        id: r.get(0)?,
        path: r.get(1)?,
        title: r.get(2)?,
        album: r.get(3)?,
        year: r.get::<_, Option<i64>>(4)?.map(|y| y as u16),
        bpm: r.get(5)?,
        camelot: r.get(6)?,
        key_text: r.get(7)?,
        primary_artist: r.get(8)?,
        rating: r.get::<_, i64>("rating").unwrap_or(0),
    });

    if seed_bpm.is_some() {
        let rows = stmt.query_map(params![bpm_lo, bpm_hi], row_map)?;
        for row in rows { out.push(row?); }
    } else {
        let rows = stmt.query_map([], row_map)?;
        for row in rows { out.push(row?); }
    }
    Ok(out)
}

// ─── Resume — last playlist or last-played track ───────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeState {
    pub last_track_id: Option<i64>,
    pub last_track_title: String,
    pub last_playlist_id: Option<i64>,
    pub last_playlist_name: String,
}

pub fn resume_state(conn: &Connection) -> rusqlite::Result<ResumeState> {
    let mut state = ResumeState::default();

    if let Some(s) = catalog::get_state(conn, "last_track_id")? {
        if let Ok(id) = s.parse::<i64>() {
            state.last_track_id = Some(id);
            state.last_track_title = conn.query_row(
                "SELECT title FROM tracks WHERE id = ?1",
                params![id],
                |r| r.get::<_, String>(0),
            ).unwrap_or_default();
        }
    }
    if let Some(s) = catalog::get_state(conn, "last_playlist_id")? {
        if let Ok(id) = s.parse::<i64>() {
            state.last_playlist_id = Some(id);
            state.last_playlist_name = conn.query_row(
                "SELECT name FROM playlists WHERE id = ?1",
                params![id],
                |r| r.get::<_, String>(0),
            ).unwrap_or_default();
        }
    }
    Ok(state)
}

pub fn record_play(conn: &Connection, track_id: i64) -> rusqlite::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    conn.execute(
        "UPDATE tracks SET last_played_at = ?1, play_count = play_count + 1 WHERE id = ?2",
        params![now, track_id],
    )?;
    catalog::set_state(conn, "last_track_id", &track_id.to_string())?;
    Ok(())
}
