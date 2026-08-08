# Cross-network peers: what exists, and what I got wrong about it

## The correction

I said last commit that `snapclone-dht` is the crate logscope should depend on for DHT discovery.
That is true for **identity** and wrong for **the DHT**.

```
snapclone-dht/src/identity.rs    real — ScId generation, parsing, 3 tests (now passing)
snapclone-dht/src/announce.rs    7 lines, `// TODO: implement via OpenDHT FFI or HTTP announce`
snapclone-dht/src/gossip.rs      39 lines, scaffold
snapclone-dht/src/resolver.rs    2 lines, a re-export

apps/snapclone/src/network/MicroDhtClient.cpp    209 lines, Qt UDP client for a TEST node
apps/snapclone/src/network/DhtService.cpp         76 lines
```

**The Rust side has identity and a stub. The working DHT is C++ and Qt-bound**, which makes it
unusable from logscope without dragging Qt in — and logscope's tests cannot even run in a container
that has Qt missing, which is how the `table_ops` duplicate happened earlier.

## The three real options

**1. Port `MicroDhtClient` to Rust.** 209 lines of async UDP against a known test node, and the Qt
parts (`QUdpSocket`, `QTimer`) map onto tokio directly. This is the smallest thing that gives
logscope a working lookup, and it also frees snapclone from Qt on that path.

**2. Use a crates.io DHT.** Mainline-BitTorrent DHT crates exist and are mature. Wrong network
though: peers would be discoverable by anyone crawling the DHT, and the identity scheme would not be
snapclone's.

**3. FFI to opendht.** The most capable and the largest, and it puts a C++ build dependency in
logscope's path — which is exactly the property that has already cost this project a duplicate
module.

## What is already built and waiting

`reachability.rs` models DHT-discovered peers correctly and has nothing to feed it:

* discovery and reachability kept in separate fields, because on a LAN they are the same event and
  across networks they are not
* a DHT peer that was never probed is refused rather than assigned work
* a stale record is refused even when a path once worked, since a record outlives its machine
* relayed and hole-punched paths carry what they cost

`deviceplan.rs` scores devices per task, and `remoteworkspace.rs` makes a remote build result
attributable. **The plumbing on either side of the DHT exists; the DHT does not.**

## The recommendation

Option 1, and not immediately. Porting 209 lines of UDP is a contained job with a clear finish line,
but it is worth doing AFTER the frame finding — 67% of submits with no frame open is a larger win
than any amount of cross-network scheduling, and it is on the machine that already works.

Cross-network peers make more hardware available. The frame bug means the hardware in front of you is
running at a fraction of what it can.
