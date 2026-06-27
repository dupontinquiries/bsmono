// gitmind-core/src/lib.rs
//
// Pure-Rust core for a touch-first, advanced repo manager. No Qt here —
// the gui/tui/cli crates wire this up. Everything in this crate is
// deterministic and testable without a display or a Qt runtime.
//
// The big idea: keep ordinary git semantics (refs, commits, branches,
// worktrees) but solve git's two real pain points — terrible large-file
// storage and a clean working tree being the enemy of "save everything
// constantly" — with two purpose-built layers:
//
//   1. A content-addressed CHUNK STORE (store.rs). Files are split by
//      content-defined chunking, each chunk is BLAKE3-addressed and
//      zstd-compressed, and identical chunks are stored once. A 2 GB
//      video re-exported with a 1-frame change re-stores ~one chunk,
//      not 2 GB. This removes the practical size ceiling and the
//      "binaries bloat the pack" problem in one move.
//
//   2. A CHECKPOINT ENGINE (checkpoint.rs). A background watcher
//      snapshots the working tree into a SHADOW ref namespace
//      (refs/checkpoints/...) without ever touching the user's index
//      or HEAD. You get "every change is saved" + a scrubbable timeline
//      while your real history stays curated. Aggressive branch
//      creation and backups are just policies over this same machinery.
//
// The git-facing operations go through a BACKEND trait (backend.rs) so
// the default subprocess implementation (honest, ecosystem-compatible,
// matches the reference's git_status.rs choice) can later be swapped or
// augmented by a hybrid that reads/writes the chunk store directly.

pub mod oid;
pub mod store;
pub mod checkpoint;
pub mod backend;
pub mod remote;
pub mod compose;
pub mod query;
pub mod review;
pub mod similarity;
pub mod watch;
pub mod maintenance;
pub mod encode;
pub mod patch;
pub mod announce;
pub mod launch;
pub mod select;
pub mod portage;
pub mod ingest_zip;
pub mod provenance;
pub mod bridge;
pub mod watcher;
pub mod fanotify;
pub mod gmignore;
pub mod watch_session;
pub mod diff;
pub mod gitobj;
pub mod native;
pub mod varcache;
pub mod binpin;
pub mod checks;

// Re-exports for the common path so frontends import one place.
pub use backend::{RepoBackend, GitCli, RepoStatus};
pub use checkpoint::{CheckpointEngine, Checkpoint};
pub use oid::Oid;
pub use store::{ChunkStore, BlobManifest, IngestReport};

/// Crate version, surfaced by frontends in their build banner (the
/// reference prints a version + build-timestamp banner at startup).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
