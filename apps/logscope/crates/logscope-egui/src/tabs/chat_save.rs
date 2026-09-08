//! chat_save — the on-disk shape of a conversation, and the conversions to it.
//!
//! Third cut of `chat.rs`. `MsgSave`/`ChatSave` are the SERIALISED forms and
//! `msg_to_save`/`save_to_msg` are the only two functions that know how a live `Msg` maps
//! onto them — which is exactly the pair you want to read together when the autosave format
//! changes, and which were four hundred lines apart from the code that calls them.
//!
//! The `#[serde(default)]`s here carry a rule worth keeping in one place: an autosave that
//! refuses to parse costs the operator their conversation, which is far worse than a lost
//! field. Every field added after the first release therefore has a default, and a missing
//! one loads as "unknown" rather than as a precise wrong value.
//!
//! `pub(super)` throughout: this is `chat`'s format, not a public one. A child module, so
//! `use super::*` reaches `Msg`, `Role` and the rest without re-importing each.

use super::*;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub(super) struct MsgSave {
    pub(super) role: u8,
    pub(super) variants: Vec<String>,
    pub(super) cur: usize,
    /// Wall-clock seconds when the turn happened.
    ///
    /// `#[serde(default)]` for the same reason `replicator` below has one: an
    /// autosave that refuses to parse costs the operator the conversation,
    /// which is a far worse failure than a lost timestamp. Saves written before
    /// this field load with `0`, which renders as "unknown" rather than as a
    /// precise wrong date.
    #[serde(default)]
    pub(super) at: u64,
}
/// A named, restorable snapshot of the whole conversation.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub(super) struct Checkpoint { pub(super) label: String, pub(super) at_unix: u64, pub(super) msgs: Vec<MsgSave> }
/// What gets written to disk on every turn — the live conversation plus its checkpoints — so a
/// crash (e.g. an OOM mid-generation) loses nothing and the user can reopen and recover.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(super) struct ChatSave {
    pub(super) msgs: Vec<MsgSave>,
    pub(super) checkpoints: Vec<Checkpoint>,
    /// The replicator fan, when one is open. `#[serde(default)]` so every save written before this
    /// field existed still loads — an autosave that refuses to parse costs the operator the
    /// conversation, which is a far worse failure than a lost fan.
    #[serde(default)]
    pub(super) replicator: Option<logscope::replicator::ReplicatorSave>,
}

pub(super) fn msg_to_save(m: &Msg) -> MsgSave { MsgSave { role: if m.role == Role::User { 0 } else { 1 }, variants: m.variants.clone(), cur: m.cur, at: m.at } }
pub(super) fn save_to_msg(s: &MsgSave) -> Msg {
    let variants = if s.variants.is_empty() { vec![String::new()] } else { s.variants.clone() };
    let cur = s.cur.min(variants.len() - 1);
    // THE STORED STAMP, not `wall_now()`.
    //
    // This is what makes the clock survive a reload. Taking the current time
    // here would restamp every turn of a week-old conversation to the moment it
    // was opened, so the transcript's "Started" line would say today, the
    // between-turn gaps would all collapse to zero, and the prompt's time line
    // would report a long session as brand new — the exact question it was
    // added to answer.
    //
    // `0` for a save written before this field existed. `stamp_utc` renders
    // that as "unknown" and the gap logic skips it, which is the honest
    // outcome: those turns have no recorded time and inventing one would make
    // an old transcript look precisely dated.
    Msg { role: if s.role == 0 { Role::User } else { Role::Assistant }, variants, cur, born: Instant::now(), at: s.at, editing: false, edit_buf: String::new(), think_open: true, parent: None , model: None }
}
