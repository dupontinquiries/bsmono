//! dht — the last step the README's honest-status list names directly:
//! "session keys, chunking and reassembly are done and tested against the
//! awkward carrier shape; what remains is calling snapclone-dht's put/get
//! from a running node."
//!
//! `carrier` already derives the session key, splits a [`Packet`](crate::wire::Packet)
//! into records that fit a DHT value, and reassembles them tolerating
//! duplication and reordering. None of that touches storage. This module is
//! the two functions that do: [`publish`] turns a packet into `Store::put`
//! calls, [`discover`] turns a `Store::get` into whatever packets happen to
//! be complete right now.
//!
//! ## Why one call site can name `snapcore::store::Store` directly
//!
//! `Store` is snapcore's key → multi-value, TTL-expiring, re-announce-aware
//! map — already built, already tested (`shared/snapcore`), and exactly the
//! shape `carrier`'s own docs specify a DHT value store needs. A **running**
//! node still answers STORE/FIND_VALUE by delegating to one of these; the
//! part genuinely missing is the network half — iteratively finding, and
//! replicating to, the several nodes *closest* to a key, which is
//! `share-kit`'s still-pending "networked `Backend`" (see the monorepo
//! `TODO.md`), not something to reinvent here. What this module gives is the
//! same thing `studio-io`'s `TestIo` gives the render path: a real,
//! honestly-scoped implementation of the *local* half of the seam, provable
//! end-to-end without a network, with the network-replication half staying
//! exactly as unbuilt — and exactly as named — as it was before this patch.
//!
//! ## Why chunks from unrelated messages can share one key safely
//!
//! `publish` stores every chunk of one packet as its own **distinct value**
//! under the session key, using `Store`'s existing multi-value-per-key
//! support rather than inventing a second index. Two `publish` calls for two
//! different packets (or a re-broadcast after an edit) simply add more
//! values under the same key. `discover`'s `Reassembler` already partitions
//! incoming chunks by `msg` — feeding it a get() that mixes several
//! in-flight messages together is exactly the "unordered, lossy carrier"
//! shape `carrier`'s own tests already exercise; this module only adds where
//! the bytes came from.

use crate::carrier::{session_key as derive_session_key, Chunk, Reassembler};
use crate::wire::Packet;
use snapcore::routing::NodeId;
use snapcore::store::Store;

/// How long one published chunk lives before it must be re-announced.
/// Generous relative to a normal edit cadence — the point is that a joiner
/// who is offline for a while still finds a recent catch-up, not that every
/// keystroke re-publishes; the caller decides re-announce cadence by calling
/// [`publish`] again with the same bytes, which `Store::put` treats as a
/// TTL refresh rather than a duplicate.
pub const DEFAULT_TTL_TICKS: u64 = 300;

/// Publish `packet` under `session_key`: one `Store` value per carrier chunk.
/// `msg` must differ from any other in-flight publish under the same key
/// (same requirement `carrier::split` already documents) so `discover`'s
/// `Reassembler` doesn't interleave two packets' pieces.
pub fn publish(store: &mut Store, session_key: [u8; 20], packet: &Packet, msg: u64, ttl_ticks: u64, now: u64) {
    let key = NodeId(session_key);
    for chunk in crate::carrier::split(packet, msg) {
        store.put(key.clone(), chunk.encode(), ttl_ticks, now);
    }
}

/// Read everything currently live under `session_key` and return every
/// packet that has all its chunks present. Bytes that don't decode as a
/// [`Chunk`] are skipped rather than treated as fatal: a DHT value store is
/// an untrusted, best-effort carrier (`carrier`'s own module docs), so
/// garbage under a guessed-at key is an expected shape to tolerate, not a
/// bug to panic over.
pub fn discover(store: &Store, session_key: [u8; 20], now: u64) -> Vec<Packet> {
    let key = NodeId(session_key);
    let mut r = Reassembler::default();
    let mut out = Vec::new();
    for bytes in store.get(&key, now) {
        if let Some(chunk) = Chunk::decode(&bytes) {
            if let Some(p) = r.feed(chunk) {
                out.push(p);
            }
        }
    }
    out
}

/// Convenience: the session key derived the same way [`crate::session::Team::session_key`]
/// does, for callers that only have the raw session name (e.g. a test, or a
/// tool that doesn't have a `Team` in hand).
pub fn key_for(session: &str) -> [u8; 20] {
    derive_session_key(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Doc, SiteId, Stamp, StampedOp};
    use studio_core::{Clip, ClipId, Op, Project, SourceId, Ticks, Track, TrackId, TrackKind};

    fn big_catchup(n: u64) -> Packet {
        Packet::Catchup {
            ops: (0..n)
                .map(|i| StampedOp {
                    stamp: Stamp { lamport: i + 1, site: SiteId(1) },
                    op: Op::AddClip {
                        clip: Clip {
                            id: ClipId(i),
                            source: SourceId(1),
                            track: TrackId(1),
                            pos: Ticks::from_seconds(i as f64),
                            src_in: Ticks::ZERO,
                            len: Ticks::from_seconds(1.0),
                            gain: 1.0,
                            effects: Vec::new(),
                        },
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn a_small_packet_round_trips_through_the_store() {
        let mut store = Store::new();
        let key = key_for("team-a/cut");
        let p = Packet::Hello { site: SiteId(1), since: 0 };
        publish(&mut store, key, &p, 1, DEFAULT_TTL_TICKS, 0);
        assert_eq!(discover(&store, key, 0), vec![p]);
    }

    #[test]
    fn a_multi_chunk_catchup_survives_the_store_round_trip() {
        // The case the whole module exists for: a joiner's catch-up is too
        // big for one value, same as `carrier`'s own tests establish.
        let mut store = Store::new();
        let key = key_for("team-a/cut");
        let p = big_catchup(400);
        publish(&mut store, key, &p, 7, DEFAULT_TTL_TICKS, 0);
        assert_eq!(discover(&store, key, 0), vec![p]);
    }

    #[test]
    fn two_concurrent_publishes_under_one_key_do_not_interleave() {
        // `Reassembler` already partitions by `msg`; this pins that the
        // property survives going through `Store`'s multi-value list too —
        // storage order is not delivery order, same reasoning `carrier`'s
        // own "chunks arriving backwards and twice" test exists for.
        let mut store = Store::new();
        let key = key_for("team-a/cut");
        let a = big_catchup(120);
        let b = Packet::Hello { site: SiteId(2), since: 4 };
        publish(&mut store, key, &a, 1, DEFAULT_TTL_TICKS, 0);
        publish(&mut store, key, &b, 2, DEFAULT_TTL_TICKS, 0);

        let mut got = discover(&store, key, 0);
        got.sort_by_key(|p| format!("{p:?}"));
        let mut want = vec![a, b];
        want.sort_by_key(|p| format!("{p:?}"));
        assert_eq!(got, want, "both packets reconstruct, neither corrupts the other");
    }

    #[test]
    fn re_publishing_the_same_packet_refreshes_rather_than_duplicates() {
        // `Store::put`'s re-announce path (identical bytes -> TTL refresh,
        // not a new value) is exactly what a live session repeatedly
        // re-broadcasting the same catch-up should get for free.
        let mut store = Store::new();
        let key = key_for("team-a/cut");
        let p = Packet::Hello { site: SiteId(1), since: 0 };
        publish(&mut store, key, &p, 1, 100, 0);
        publish(&mut store, key, &p, 1, 100, 50); // re-announce at t=50 -> expires at 150
        assert_eq!(store.value_count(&NodeId(key)), 1, "no duplicate value from the re-announce");
        assert_eq!(discover(&store, key, 120), vec![p], "still live past the original TTL");
    }

    #[test]
    fn an_unpublished_key_discovers_nothing() {
        let store = Store::new();
        assert!(discover(&store, key_for("nobody/here"), 0).is_empty());
    }

    #[test]
    fn expired_chunks_are_not_discovered() {
        let mut store = Store::new();
        let key = key_for("team-a/cut");
        let p = big_catchup(120); // multi-chunk, so a partial expiry would also corrupt it
        publish(&mut store, key, &p, 1, 100, 0); // expires at 100
        assert_eq!(discover(&store, key, 50), vec![p.clone()], "still live");
        assert!(discover(&store, key, 100).is_empty(), "gone at expiry");
    }

    #[test]
    fn garbage_under_the_key_is_skipped_not_fatal() {
        // A DHT value store is untrusted and best-effort — `carrier`'s own
        // module docs say so. Something that isn't a chunk at all (or is a
        // stray value some other feature put under a colliding key) must not
        // panic `discover`.
        let mut store = Store::new();
        let key = key_for("team-a/cut");
        store.put(NodeId(key), vec![1, 2, 3], 100, 0); // too short to be a Chunk header
        let p = Packet::Hello { site: SiteId(1), since: 0 };
        publish(&mut store, key, &p, 1, 100, 0);
        assert_eq!(discover(&store, key, 0), vec![p], "the real chunk still reconstructs");
    }

    #[test]
    fn two_peers_share_a_session_key_without_being_told_where_it_lives() {
        // The property `session::Team::session_key` exists for, exercised at
        // this module's boundary: both sides compute the same key from the
        // same session name independently.
        assert_eq!(key_for("team-a/cut"), key_for("team-a/cut"));
        assert_ne!(key_for("team-a/cut"), key_for("team-b/cut"));
    }

    #[test]
    fn a_real_session_broadcast_publishes_and_a_joiner_discovers_it() {
        // End to end through the actual `Session` API a caller would use,
        // not just synthetic packets.
        let mut host = crate::session::Session::open(
            crate::session::Team::from_contacts("cut", &["sc-a".into()]),
            "proj",
            Doc::new(SiteId(1), Project { name: "cut".into(), ..Default::default() }),
        );
        host.doc
            .local(Op::AddTrack {
                track: Track { id: TrackId(1), kind: TrackKind::Video, name: "V1".into(), muted: false, locked: false, route: None },
                at: 0,
            })
            .unwrap();

        let mut store = Store::new();
        publish(&mut store, host.key(), &host.broadcast(), 1, DEFAULT_TTL_TICKS, 0);

        let mut joiner = Doc::new(SiteId(2), Project { name: "cut".into(), ..Default::default() });
        for p in discover(&store, host.key(), 0) {
            joiner.receive(p);
        }
        assert_eq!(joiner.project, host.doc.project, "the joiner rebuilt the session purely from what was published");
    }
}
