//! Session persistence — saves the writer's current state so closing the app
//! doesn't lose work.
//!
//! Atomic-rename pattern: write to `session.json.tmp` first, then `rename` it
//! over `session.json`. A crash mid-write leaves the previous good session
//! intact rather than half-written garbage.
//!
//! Shape kept deliberately minimal. Audio paths and recorded fragments will
//! join this struct when Path A lands; the schema version lets us evolve.

use serde::{Deserialize, Serialize};

use crate::vibe::VibeSummary;

const SESSION_PATH: &str = "session.json";
const TMP_PATH: &str = "session.json.tmp";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema: u32,
    pub vibe: VibeSummary,
    pub bpm_input: String,
    pub syll_input: String,
    pub stress_input: String,
    pub seed_input: String,
    pub lyrics: String,
}

impl Session {
    pub fn from_vibe(vibe: VibeSummary, lyrics: String) -> Self {
        let bpm_input = format!("{}", vibe.bpm as u32);
        let syll_input = format!("{}", vibe.syllables_per_bar);
        let stress_input = crate::vibe::render_stress_pattern(&vibe.stress_pattern);
        let seed_input = vibe.rhyme_seeds.first().cloned().unwrap_or_default();
        Self {
            schema: SCHEMA_VERSION,
            vibe,
            bpm_input,
            syll_input,
            stress_input,
            seed_input,
            lyrics,
        }
    }
}

/// Load `session.json` if present and parseable. Returns `None` on any failure
/// — first-run users see defaults, never an error dialog.
pub fn load() -> Option<Session> {
    let raw = std::fs::read_to_string(SESSION_PATH).ok()?;
    let session: Session = serde_json::from_str(&raw).ok()?;
    if session.schema != SCHEMA_VERSION {
        // Future migrations would happen here. For now, ignore old schemas.
        eprintln!(
            "kRapper: ignoring session.json with schema {} (expected {SCHEMA_VERSION})",
            session.schema
        );
        return None;
    }
    Some(session)
}

/// Save the session. Returns `Err` only for the caller to log; the app should
/// keep running regardless of save failure.
pub fn save(session: &Session) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(TMP_PATH, json.as_bytes())?;
    std::fs::rename(TMP_PATH, SESSION_PATH)?;
    Ok(())
}
