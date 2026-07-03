//! snapclone-mediashare — shared-conversation distribution for group media.
//!
//! Group images/videos are large and a conversation is SHARED: every member
//! already holds the back-conversation, so the biggest saving is not resending
//! what the group already has. This crate provides that layer, independent of
//! the codec (snapclone-vqcodec et al.) and the transport:
//!
//!   * chunk    — content-addressed chunking + dedup + verified reassembly.
//!                A blob becomes an ordered list of SHA-256-addressed chunks;
//!                members fetch only the chunks they lack, so re-sent media and
//!                media sharing blocks cost (almost) nothing.
//!   * keyshare — multiway key distribution: a per-media content key wrapped to
//!                every group member with ephemeral X25519 + ChaCha20-Poly1305.
//!
//! Both are pure logic / standard crypto and fully unit-tested. The codec, the
//! face/landmark-driven framing, and the transport are separate seams.

pub mod chunk;
pub mod keyshare;
pub mod strip;
