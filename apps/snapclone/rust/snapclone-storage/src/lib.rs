//! snapclone-storage — SQLite contact and snap persistence.
//!
//! Rust port of StorageManager.cpp using rusqlite instead of Qt's QSqlDatabase.
//!
//! # Key design decisions (matching the C++ discussion comments)
//!
//! ## Journal mode: DELETE + synchronous=FULL
//! WAL mode was the original choice but causes silent data loss on mobile:
//! if SIGKILL fires before a WAL checkpoint, unsynced writes are in the WAL
//! file which may be lost if the filesystem flushes incompletely.
//! DELETE journal + synchronous=FULL fsyncs every commit to the main file.
//!
//! ## INSERT OR REPLACE for contacts
//! The old C++ code used INSERT OR IGNORE + UPDATE which had multiple failure
//! paths. INSERT OR REPLACE is atomic, works on all SQLite versions, and
//! preserves the simpler "read existing fields → merge → replace" pattern.
//!
//! ## Porting status
//!
//! | C++ method                      | Rust location           | Status   |
//! |---------------------------------|-------------------------|----------|
//! | StorageManager::open()          | Db::open()              | DONE     |
//! | applyMigrations()               | migrations::apply()     | DONE     |
//! | saveContact()                   | Db::save_contact()      | DONE     |
//! | loadContacts()                  | Db::load_contacts()     | DONE     |
//! | loadContact()                   | Db::load_contact()      | DONE     |
//! | saveSnap()                      | Db::save_snap()         | scaffold |
//! | loadSnap()                      | Db::load_snap()         | scaffold |
//! | undeliveredSnapIds()            | Db::undelivered_snaps() | scaffold |
//! | markSnapDelivered()             | Db::mark_delivered()    | scaffold |

pub mod contact;
pub mod snap;
pub mod migrations;
pub mod db;

pub use db::Db;
pub use contact::{Contact, ContactState};
pub use snap::{Snap, SnapId};
