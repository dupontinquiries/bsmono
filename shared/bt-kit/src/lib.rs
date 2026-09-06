pub mod protocol;

#[cfg(feature = "rfcomm")]
pub mod rfcomm_io;

// Re-exported, not left for downstream crates to depend on separately:
// anything using `rfcomm_io`'s `Stream` also needs `bluer`'s own types
// (`Address`, `SocketAddr`, ...) and `tokio` to drive it, and two
// independent `Cargo.toml` entries for the same crate risk drifting to
// different versions of bluer/tokio between this crate and its caller —
// exactly the kind of mismatch that compiles until it doesn't.
#[cfg(feature = "rfcomm")]
pub use bluer;
#[cfg(feature = "rfcomm")]
pub use tokio;

pub use protocol::{Chunk, Frame, FrameReader, Manifest, Reassembler, DEFAULT_CHUNK_SIZE};
