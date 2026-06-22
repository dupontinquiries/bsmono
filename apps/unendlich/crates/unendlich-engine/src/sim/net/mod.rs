//! Networking for the deterministic sim — brick 4.
//!
//! Three layers, smallest trusted surface at the bottom:
//!
//! - [`wire`] — pure (de)serialization of input and sync packets. No I/O,
//!   never panics on bad input, allocation-bounded. Fully unit-tested.
//! - [`session`] — pure session logic over [`crate::sim::rollback::Rollback`]:
//!   redundant resend for loss tolerance, ack-based pruning, feeding remote
//!   inputs into the engine, and desync detection. Transport-agnostic, tested
//!   against an in-memory channel.
//! - [`transport`] — the thin I/O shell: a [`transport::Transport`] trait, a
//!   std-only non-blocking [`transport::UdpTransport`], and an in-memory
//!   [`transport::loopback_pair`] for tests/local play.
//!
//! Design choices: UDP (rollback wants the freshest inputs, not in-order
//! reliability) with application-level **redundancy** instead of
//! retransmission; only deterministic, confirmed-tick checksums are exchanged
//! for desync detection. Nothing here runs until the game loop constructs a
//! [`session::Session`] + a transport — single-player is unaffected.
//!
//! Per-frame wiring the binary is expected to do:
//! 1. `session.record_local(tick, input)` + `rb.set_local_input(local, input, world)`
//! 2. `transport.send(&session.outgoing_input_packet(rb.confirmed_tick())?)`
//! 3. `transport::drain(&transport, &mut session, &mut rb, world)`
//! 4. `rb.advance(world)` then `session.note_confirmed(&rb)`
//! 5. occasionally `transport.send(&session.outgoing_sync_packet(&rb))`

pub mod codec; // r340-sl-net: shared wire codec
pub mod chunk_stream; // r332: world-streaming chunk record codec (item 6, §B)
pub mod chunk_compress; // r387-mp: RLE payload codec + seal/open path for chunk_stream
pub mod rendezvous_join; // r388-mp: bridge dht_mailbox Accept → net_rendezvous hole-punch
pub mod mp_dht_mailbox; // r389-mp: MailboxDht backed by mp_dht Mailboxes (real routing model)
pub mod entity_replication; // r390-mp: host seal + client receive over entity_snapshot
pub mod world_cmd_wire; // r391-mp: WorldCmd reliable-channel codec + remote apply
pub mod presence_loop; // r392-mp: host re-announce + browser classify/evict over WorldAdvert
pub mod client_join; // r393-mp: client join driver (mailbox handshake → hole-punch → connected)
pub mod dht_mailbox;
pub mod entity_snapshot; // r330: host-authoritative entity transform replication (§C, item 7)
pub mod identity;
pub mod session;
pub mod transport;
pub mod wire;
pub mod world_advert; // r334: DHT world-advert wire codec (item 1, §A)
pub mod world_presence; // r336: host re-announce + browser TTL staleness (item 3, §A)

pub use session::{Ingested, Session};
pub use transport::{drain, loopback_pair, LoopTransport, Transport, UdpTransport};
pub use wire::{decode, Packet, PROTOCOL_MAGIC, PROTOCOL_VERSION};
pub use world_advert::{decode_advert, encode_advert, AdvertWire, CandWire};
pub use world_presence::{freshness, prune_stale, Freshness, PresenceBeacon};
pub use identity::{JoinRequest, Mailbox, NameRegistry, ReqState, Subscribers, UnendlichId, Whitelist};
pub use chunk_stream::{decode_record, encode_record, select_plan, apply_delta, build_delta, ChunkRecord, SendPlan};
pub use dht_mailbox::{host_accept, host_deny, host_drain_requests, mailbox_key, poll_response, send_request, JoinOutcome, LocalDht, MailMsg, MailboxDht, MailboxKey};
pub use entity_snapshot::{decode_snapshot, encode_snapshot, AuthorityStamper, EntityKind, EntityState, Snapshot as EntitySnapshot, SnapshotGate};
