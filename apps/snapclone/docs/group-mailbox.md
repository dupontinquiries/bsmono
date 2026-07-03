# Group-shared DHT mailbox — design + rollback

## What this is

A single shared DHT key per group, `H(group_id + "/groupmsg")`, that every
member reads and writes — instead of each sender fanning out one write per
recipient. One lookup is amortized across the whole group, cutting DHT traversal
for group traffic.

The key insight ("share the slot, not the key"): the slot LOCATION is public and
shared, but each member's message is sealed under their OWN per-sender ratchet
(`snapclone-group::ratchet::SenderChain` + `seal`), NOT a shared group symmetric
key. That preserves the properties we already pay for in the per-peer path:

- **Forward secrecy** — per-sender ratchet; one key compromise doesn't open past
  traffic.
- **Revocation** — drop a member by refusing their sender key; no group-wide key
  rotation.
- **Blast radius** — one member's device leak exposes only their own sender
  chain, not the whole group.

A shared symmetric group key would have been simpler but would have thrown away
all three. We deliberately did not do that.

## Why it doesn't hurt DHT resiliency

OpenDHT over-replicates popular keys — a key many nodes read gets cached more
widely, not less. A shared slot improves availability for that key. The only
thing it stresses is multi-writer storage, and OpenDHT already stores distinct
values side-by-side under one key (content-derived value-ids), which the
per-recipient `connreq` slot has always relied on. Verified in
`JamiAdapter_dht_impl.cpp`: `publishConnectionRequest` uses `dht_value_new`
(auto value-id) and `checkConnectionRequests`' GET callback returns `true` to
iterate ALL values, deduping byte-identical replicas. So N senders coexist.

## Where the logic lives

- `rust/snapclone-group/src/mailbox.rs` — pure, tested:
  - `MailboxMode::from_flag(bool)` → `PerRecipient` (default/rollback) | `GroupShared`.
  - `group_slot_key(group_id)` → `"group_id/groupmsg"`.
  - `MailboxEntry { sender_id, group_id, seq, sealed }` + `to_bytes`/`from_bytes`
    (plaintext addressing header, sealed body; mirrors the existing connreq blob
    layout). Capped at `MAX_ENTRY_BYTES` (2048, matches the C++ GET cap).
  - `SlotReader` — collects GET values, dedups replicas, filters to the group,
    orders by `(sender_id, seq)` (stable across peers), reports per-sender `gaps`.
  - `build_entry(...)` — seal content under a `SenderChain` message key, wrap as
    an entry.
- `rust/snapclone-group/src/ffi.rs` — C ABI: `sc_mailbox_mode_from_flag`,
  `sc_mailbox_slot_key`, `sc_mailbox_build_entry`, `sc_mailbox_parse_entry`.
- `include/group/snapclone_group_ffi.h` — declarations (verified C + C++).
- `src/network/JamiAdapter_dht_impl.cpp` — multi-writer coexistence logging (see
  below). The actual PUT/GET on the group slot is the C++ integration step still
  to be wired (the per-recipient path is untouched and shipping).

Tests: `snapclone-group` 73 (incl 10 mailbox + 4 mailbox-FFI). The C++ consumer
demo (`/tmp/mbx_demo.cpp` during development) linked the real staticlib and
round-tripped build→parse.

## Logging added (for the hardware test)

`snapclone.dht`, on every mailbox GET completion:

```
Mailbox check complete for "sc-XXXX" — distinct values: N (replica copies collapsed: M)
```

- `distinct values: N` — how many DIFFERENT senders' values coexisted under the
  one key. For the per-recipient connreq slot this is normally 0–1. **If you see
  N > 1, multi-writer coexistence is confirmed on real hardware** — the property
  the shared slot depends on.
- `replica copies collapsed: M` — byte-identical copies OpenDHT returned
  (replication factor); collapsed to one before forwarding.

Grep target: `Mailbox check complete .* distinct values`.

## ROLLBACK (next hardware test)

The feature is OFF by default and gated by ONE flag. Nothing about the shipping
per-recipient path changes while it's off; there is no data migration and no
format change to the existing connreq slot.

**Flag:** QSettings `[network] groupMailboxShared`, default `false`.
In `~/.config/SnapCloneProject/SnapClone.conf`:

```ini
[network]
groupMailboxShared=true   ; enable; remove or set false to roll back
```

- `false` (default) → `MailboxMode::PerRecipient`. Senders fan out per-recipient
  exactly as today. The group slot is neither written nor read.
- `true` → `MailboxMode::GroupShared`. Members publish per-sender entries to the
  shared slot.

**To roll back after the test:** set `groupMailboxShared=false` (or delete the
line). No rebuild required — it's a runtime flag. The diagnostic logging above
stays (harmless, info-level) and keeps reporting `distinct values` for the
per-recipient slot, which is still useful.

If a build-level rollback is ever needed, the mailbox module is self-contained
(`src/mailbox.rs` + the `sc_mailbox_*` FFI + the header block + the GetCtx
counters) and removing it doesn't touch the per-recipient connreq path.
