# Backup, rollback & cross-device sync

How the requested backup folder, system rollbacks, encrypted shared
backups, the connection manager, LAN/Bluetooth pairing, the future DHT,
and Proton Drive all fit together. Everything rides on one fact: the data
is already content-addressed chunks (`store.rs`), so backup = "copy the
chunks a target is missing" and restore = "fetch the chunks you lack."

## One seam for every target — `remote.rs`

`RemoteTarget` is the single interface. Concrete impls, in build order:

1. **LocalFolder** — the "backup folder that compresses hard." It's just
   the chunk store (zstd + dedup) mirrored to another disk/mount, plus
   the ref set. Hardest-compressing, cheapest, same-machine.
2. **RcloneRemote** — wrap the `rclone` binary for cloud targets. This is
   the honest answer to "Proton Drive integration": Proton has **no
   stable official API** for standalone third-party apps yet — their SDK
   preview (Jan 2026) explicitly isn't ready for standalone apps and
   doesn't expose third-party auth; CLI tooling is only slated for
   Apr–Jun 2026. rclone already ships a community Proton Drive backend
   (reverse-engineered, beta) *and* S3, Backblaze B2, Dropbox, Google
   Drive, etc. Wrapping rclone gives you Proton Drive **and every other
   cloud at once**, and insulates you from Proton's churn. Far better than
   coding against Proton directly today.
3. **LanPeer** — trusted device on the same network, bulk transfer over
   QUIC.
4. **DhtTarget** — your future DHT slots in here as one more impl. Design
   it as content-addressed fetch (ask the DHT "who has chunk <oid>"),
   which maps perfectly onto the chunk model.

## Backup folder that "compresses hard"

The store already does FastCDC dedup + zstd level 9. For cold backups we
can go further: zstd level 19–22 with a trained dictionary over small
chunks, and (later) re-pack many small chunks into larger zstd frames for
better ratio on archival targets. Media keeps its lossless originals;
proxies are regenerable and need not be backed up.

## System rollbacks + corruption resilience

- **Rollback** is the checkpoint engine in reverse: every snapshot is a
  full tree, so "roll the whole repo back to 14:32" = `materialize()` that
  checkpoint into the worktree (side-by-side first, then swap). No data is
  destroyed — the prior state is itself a checkpoint.
- **Corruption detection**: chunks are BLAKE3-addressed, so verifying is
  free — recompute the hash, compare to the key. A scrub pass flags any
  chunk whose bytes no longer match its Oid, and self-heals by pulling a
  good copy from any `RemoteTarget` that has it.
- **"Smart memory management"**: the store is mmap'd (redb) and ingest/
  restore stream, so a scrub or rollback of a 50 GB repo runs in bounded
  RAM. GC keeps recent checkpoints dense and thins old ones (e.g. all for
  24 h, then hourly, then daily).
- **Concurrency safety**: a key lesson from jujutsu — design so that
  syncing the store via a dumb file copy / Dropbox / rsync can never
  corrupt it; the worst case is a detected divergence to reconcile, not a
  broken repo. redb's transactional writes + append-only chunk semantics
  (a chunk Oid's bytes never change) give us this.

## Encrypted shared backups across trusted devices

- **Encryption**: age / X25519 (`rage` crate). Each repo has a key; each
  trusted device holds a share. Off-device targets store **ciphertext
  only** — a peer or cloud keeps your chunks without being able to read
  them. Local folders can stay plaintext (still compressed).
- **Encrypt-then-dedup caveat**: naive per-recipient encryption kills
  cross-device dedup (same plaintext → different ciphertext). Fix:
  convergent encryption — derive each chunk's key from its own BLAKE3
  hash, so identical chunks encrypt identically and still dedup. (Tradeoff:
  convergent encryption leaks equality of chunks; acceptable for a
  trusted-device mesh, configurable off for paranoia.)

## Connection manager + pairing (GUI)

- A QML **connection manager**: list targets, their trust state, last
  sync, free space, and a pair button — built on the reference's
  `GestureHost`/`SlidePopup` patterns. The reference already has
  `peer_handshake.rs` / `peer_registry.rs` / multi-device-share groundwork
  to borrow from.
- **Discovery**: mDNS on the LAN (Syncthing-style).
- **Key exchange / pairing**: a Noise handshake; the out-of-band channel
  is either a short code shown on both screens or **BLE proximity**.
- **Bluetooth, honestly**: BLE is great for *pairing and discovery* and
  terrible for *bulk transfer* (single-digit Mbps). So Bluetooth pairs
  devices and exchanges keys; **WiFi LAN (QUIC) moves the bytes**. Trying
  to sync media over BLE would be painfully slow — don't.

## Build order (only if you want these)

A. LocalFolder backup + scrub/verify + rollback (pure core, testable).
B. Encryption (age + convergent option).
C. RcloneRemote (cloud incl. Proton Drive) — shell-out, low effort.
D. LanPeer (mDNS + QUIC + Noise) + connection-manager GUI.
E. DhtTarget — when your DHT exists.
