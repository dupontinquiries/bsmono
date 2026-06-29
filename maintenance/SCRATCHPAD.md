# Scratchpad / backlog

Living capture of jotted ideas, organized as we go. Newest raw notes land in
"Inbox"; once triaged they move into a themed section with a status and a plan.

Status legend: `idea` · `queued` · `in-progress` · `blocked` · `done` · `parked`

---

## Inbox (raw, untriaged)

_(empty — current dump triaged below)_

---

## A. AI-agent documentation unification

- **A1 — Unify the AI-agent docs across projects.** `in-progress`
  Inventory done (~40 docs, heavily clustered in `apps/unendlich/handoff/`).
  Catalog + unification plan now live at **`docs/agents/INDEX.md`**. Next:
  the link scheme so **web2qml/voiceterm users navigate via ZBrackets**.
- **A2 — "Claude-specific vs general-LLM" notes file.** `done`
  Written: `docs/agents/claude-vs-general.md` — Claude-specific vs general-LLM
  guidance + the logscope tree-based-development mapping; ZBracket cross-links.
- **A3 — NEWCOMER_DEVELOPER.md (what each build-system tool does).** `done`
  Root-level map of the entry points (verify.sh, build/build.sh, flatpak.sh,
  gitmind-sync.sh), the manifests, the suspenders, and "first things to run".

## B. Tooling & maintenance dir (this directory)

- **B1 — Install dev tools.** `in-progress` (see `maintenance/TOOLING.md`)
  Wanted: sccache, mold, tokei + others. Rust preferred for anything we build;
  Python acceptable. Sandbox has **no toolchain and resets each session**, so
  installs here are ephemeral — the durable output is the documented install
  spec for hardware/CI. Release-binary fetch is firewalled in-sandbox.
- **B2 — Dual toolchain 1.75 + 1.96.** `queued`
  User supplied an offline 1.96 dist; 1.75 needed too (e.g. kanal-pre8 wants
  rustc < 1.81). Documented in TOOLING.md.
- **B3 — More "suspender" tooling, Rust-first, in `maintenance/`.** `idea`
  Quick-check common Rust mistakes; expand peek/rustcheck coverage repo-wide.
  Candidate: a small Rust `repo-doctor` that wraps the static checks.
- **B4 — Recommend additional tools, doc in the Claude AI docs.** `done→living`
  See `maintenance/TOOLING.md` (recommended set + rationale).
- **B5 — Nix tooling (discussion open).** `idea (user interested; add as we go)`
  Strong fit for this repo's pain: many system deps (Qt6/gstreamer/vulkan/gtk4),
  the dual 1.75+1.96 toolchain, and cross targets (aarch64/x86/riscv) all want
  reproducible pinning. A `flake.nix` dev shell + cross toolchains would make the
  build host reproducible; tradeoff is the learning/maintenance curve, and
  flatpak already covers app *distribution*. User is fine adding nix artifacts
  incrementally (like the gitmind fixture). Next: seed a starter `flake.nix`
  (devShell + both rustc versions + the apt deps as nix pkgs) for discussion.

## C. snapclone FFI → full Rust (oxidation)

- **C1 — Move snapclone's C++ side to full Rust.** `idea / strategic`
  snapclone is the canonical Rust↔C++ DataBus FFI and the best place to start
  oxidizing: replace C++ with Rust, shrinking/eliminating the FFI surface.
  ~92K C++ LOC total in the tree, mostly here. Big, multi-stage; needs a
  component-by-component plan (start with leaf C++ that has a clean Rust analog).

## D. DHT decomposition + Rust rebuild

- **D1 — Networking decomposition: snapcore / crypto-kit / share-kit.** `scoped`
  Grounded boundary proposal: **`docs/architecture/NETWORKING_AND_CRYPTO.md`**.
  Findings: snapclone networking is ~42 crates, well-layered;
  `snapclone-crypto` is already best-of-breed (**hybrid ML-KEM-768 PQ**) → the
  **crypto-kit** base; `snapclone-group` = Signal-style ratchet/seal;
  `snapclone-mailbox` is the **share-kit** seam (per-app `MailboxId`, encrypted
  fixed-size envelopes, "share the slot not the key"); the `dht`/`microdht`
  split implies the DHT interface for **snapcore** (microdht = the net-sim
  harness too). unendlich has **no** net/crypto → future consumer, not a merge
  source. The node **ownership/lifecycle** layer (sleep/wake/refcount/parallel
  pool/port arbitration) **doesn't exist yet** — the main new piece snapcore
  adds; all the flags are captured in the doc. **First lift = crypto-kit** (pure
  Rust, no FFI). Aligns with the ops-doc snapcore-daemon + the integrated/lean
  `dht` feature (build-variant split).
  **Code-checked (this round):** snapclone's address-bearing rendezvous is
  **C++** (`JamiAdapter_dht_impl.cpp`: CONNREQ value `from_sc_id\nhost:port\n
  punch_host:punch_port\nts`, key `sha1(target+"/connreq")`, 10-min TTL == DHT
  TTL). So the Rust trait basis is **unendlich's pure-Rust `dht_mailbox` +
  `net_rendezvous`**, validated against snapclone's CONNREQ semantics + the
  IP-protection invariant; the C++ OpenDHT backend stays **behind the daemon**.
  The Rust mailbox crate is entry-model + the approval lifecycle (pure `&mut
  self`, idempotent → trait-clean) + crypto. **Decisions:** apps go **async**
  (no sync-compat shim in the kit — break + port them). Add a **ShuffleManager**
  in snapcore: freshness-aware republish/placement (alive subset → fresh slot
  with new TTL = the GC "new home"); onion-indirection layering = **v2** (privacy
  network-effect, but costs lookup hops). Gate it on a **`Lifetime`/`is_shortlived()`
  flag** on share-kit entries — set the flag now, implement only the ephemeral
  path. **Long-term demand is real:** logscope is a serving fleet
  (FLEET/PREWARM/SERVING + `gpu-gemm` serve/netserve) = persistent sessions;
  voiceterm wants self-hosted notes/sessions + a `hive` tool-drop
  (`voiceterm/src/sandbox.rs`). Durable elements likely host via **dufs (§H)**,
  not pure DHT.

## E. CI: network simulation + quick-check tooling

- **E1 — Network simulation for CI testing.** `idea`
  Existing material to harvest: `snapclone-network-diag`, `snapclone-net-intel`,
  a `network_sim` module in bo-suite, channel-bench's `--with-cpu-load`.
  → Could become a `maintenance/` tool or a shared test harness.
- **E2 — Quick-check common Rust-crate mistakes.** `queued` (overlaps B3)

## F. gitmind evaluation (side-by-side with git)

- **F1 — Actively use gitmind; run two side-by-side repos.** `in-progress (positive)`
  Trial 1: `gin` CLI builds clean; checkpoint + native import + **export-native
  byte-faithful**. Trial 2: zstd 9→19 saves only **1.5%** (below bar, reverted);
  delta compression is the real lever (see F3). `.gitmind` made a **permanent
  reproducible fixture** — `store.redb` gitignored, rebuilt by
  `maintenance/gitmind-sync.sh`; config + README tracked. Next trials:
  native-commit/merge/rebase correctness, watch+gc storage bounds, `graft`.
- **F3 — Delta compression in gitmind's chunk store.** `idea (real space lever)`
  The 110-vs-84 MB gap vs `.git` is delta compression, not zstd level. A
  cross-chunk/cross-object delta scheme (the store is BLAKE3-addressed, FastCDC
  64K) could close or beat it. Bigger change than a level bump — a real project.
- **F4 — gitmind branches for UI/UX base-swap experiments.** `idea`
  When weighing "who has better UI/UX, w2q or snapclone", spike it: use gitmind
  native branches / **`gin graft`** ("field-test another repo on a branch") to
  take one app's UI code as the base for another, let the agent iterate, then
  build/test/feel-out the quirks to inform the decision. graft is built for
  exactly this. Needs a Qt6 host to actually build/feel the result.
- **F5 — Live break-detection hook vs checkpoint checks.** `decided`
  For the **human dev on hardware**: `gin watch` (autosnapshot on save) + a
  build/test hook is cheap (background) and great. For the **agent (me)**: a
  live per-save hook is high-token *and* doesn't survive the sandbox reset — so
  the right model is checkpoint-based: run `verify.sh` (fast static suspenders)
  plus a targeted `cargo check`/test on the crate I touched, occasionally,
  rather than streaming on every save.
- **F6 — gitmind vs nested `.git` (cargo makes repos by default).** `idea`
  `cargo new` creates a `.git` per crate; in a monorepo we don't want nested
  repos. Quick upgrades to weigh: set `[cargo-new] vcs = "none"` (cargo config)
  so new crates don't spawn `.git`; and check how `gin import` treats any nested
  `.git` (ignore / submodule / choke) — possibly teach it to skip them.

## G. Kits, FFI boundaries, shared libs (planning)

- **G1 — Kits / FFI boundaries / shared libs.** `in-progress (steps 1-2 done)`
  First-pass plan written: **`docs/architecture/KITS_AND_BOUNDARIES.md`** (FFI
  inventory + keep/oxidize/kit-ify + build-shape impact). Sequence progress:
  ✅ registry-kit (shared/, built+tested) · ✅ peek-view reconcile (tools/peek
  lib+bins, gitmind consumes it) · next: DHT→snapcore + variants. Key seeds:
  - *Shared-lib candidates:* `peek` (already shared at tools/peek; lib API
    `view::read_to_string`), `gitmind-core` (CLI+GUI share it — the "can't drift"
    model), network-sim/`snapclone-net-intel` (→ a test harness, cf. E1),
    rustcheck's check logic, a future `snapcore-protocol` (ops-doc) crate.
  - *FFI boundaries already in the tree:* snapclone Rust↔C++ DataBus (canonical;
    oxidation target C1), voiceterm Rust↔Qt (qmetaobject cdylib), gitmind-gui
    Rust↔Qt, photosync C++ gui ↔ Rust daemon, unendlich Rust↔Vulkan/shaders.
  - *Kit candidates (from the original handoff):* experimentkit, gesturekit
    (bot-arena), the looking-glass pattern as the extraction discipline.
  - *New (this round):* unendlich's **linkme autoloader** is the **registry-kit**
    (highest-value, pure-Rust, self-contained lift); build **variants** —
    integrated snapclone+unendlich *sharing the DHT* vs *lean* playtester builds
    without snapclone bloat — reshape the build via a `dht` cargo feature + a
    top-level `shared/` layer. Sequencing is in the doc.

- **G2 — `uikit` (egui UI layer across all apps).** `idea (major)`
  Pipe **egui** across every app, replacing the Qt layer; build out the swipe
  gestures and UI feel from snapclone (swipegestures) in egui. Approach Qt→egui
  *fresh* — don't pattern-mine the older GTK→egui logscope port. Path: get apps
  onto the Rust↔Qt FFI first (via snapclone), then port to egui generally →
  `shared/uikit`. Lighter than Qt ⇒ also helps RISC-V viability. snapclone is the
  first consumer. Parallel track to the registry-kit sequence (not a blocker).
- **G3 — unendlich ⨯ logscope overlap (shared render pieces?).** `idea`
  Both are GPU/render-heavy and likely overlap now. Investigate for a shared
  render/GPU kit before extracting; feeds the LOC-down goal.

## H. Deployment / install / uninstall

- **H1 — Clean, low-effort teardown of system installs.** `idea`
  Once systemd units + virtual keyboards get installed on a host, undoing them
  should be smart and not manual. Track what each deploy installs (units, vkbd,
  firewall rules) so a single `--uninstall` reverses it. Make install/uninstall
  a **deployment stage** in the build scripts (alongside build/flatpak).
  snapclone already ships systemd units — model the teardown there first.
- **H2 — Phone installer GUI + self-hosted distribution.** `idea`
  Ship an installer graphic for the phones that pulls from a good CDN ref / IP
  hosting a basic server (**dufs** + solid firewall patches) and lets users pick
  which actions to apply. Dovetails with the ops-doc self-hosted flatpak repo.

## Build-shape & hardware reality (this round)

- **Targets corrected** in `manifest/targets.toml`: build HOST is **aarch64
  (Asahi, 16K pages)**; phone fleet is **aarch64 musl (FP5 pmOS/Alpine, ~4K
  pages)**; **x86** and **riscv** are CROSS targets (no x86 hardware) — games
  must reach x86 (added to unendlich) and eventually riscv. Mind 16K-vs-4K page
  assumptions. *Why cross + snapclone oxidation matter.*
- **RISC-V hardware:** eyeing a DC-ROMA RISC-V mainboard (III, RVA23-compliant)
  for Framework 13 to test on. Blocker is GPU & CPU perf — the lighter `uikit`
  (egui) family helps unblock the UI side; the heavy GPU work (unendlich, and
  logscope's AI-GEMM backend) will still hit RISC-V hard.
- **LOC north-star:** ~600K now; kits/FFI should trend it **down** (dedup) — a
  maintainability goal, not size for its own sake.
- **`.spv` shaders:** `*.spv` gitignore catch-all added. Committed
  `assets/shaders/*.spv` are fine; the issue is **runtime-generated** shaders
  landing in `$CWD` — they should write to a managed location
  (`$XDG_CACHE_HOME/unendlich`, i.e. `~/.cache/unendlich`) for now, with an
  installer-managed path later. Build/test scripts should also run from a scratch
  PWD. (unendlich code change — do when we touch unendlich.)
- **Per-project ↔ monorepo sync (decision):** make the **monorepo the source of
  truth** going forward; pull any standalone fixes in *once* via **`git subtree`**
  (or `josh` for ongoing bidirectional). gitmind's `graft` is for *experiments*,
  not the canonical sync. So the stale unendlich (`PROBLEMS(5)`) gets one subtree
  merge from its standalone repo, then lives here. (See report for the long form.)
- **Parallel work (ACTIVE — 2nd chat running):** the other chat got last turn's
  bundle and is doing **snapclone UI in rust+egui** (faithful look/feel, mini
  demo, logscope-derived egui components, swipe/trackpad gestures + a logscope
  reverse-port candidate) → the **uikit** lane. This chat owns snapclone
  **networking** (snapcore/crypto-kit/share-kit). Lane split = **UI ⟂ networking**
  within snapclone (different files) → low collision. `gin`'s Qt GUI is the
  fallback small-base uikit test if the snapclone-egui spike stalls.

---

## Reliability / trust ledger (what's proven vs assumed)

- `rustcheck --fast` — **trusted**. Caught the one real break (unendlich
  geom_probe.rs:99 unclosed brace); confirmed + fixed by the unendlich session.
- `peek wiring-audit` (grep form) — **over-reports**, candidates only
  (bo-suite 7/7 dead-service flags were false positives). Authority = real peek
  binary at build stage.
- `drift` — **spec-only** today (60-line playground compiles; cargo-drift
  subcommands unimplemented). The boundary-checker we want for connections.
- `gitmind` (`gin` CLI) — **reliable core** (Trial 1). Builds clean; round-trip
  git→.gitmind→git is byte-faithful. GUI crate (Qt) untested/the "broken" part.

## Portfolio LOC snapshot (~600K total, tokei on the user's aarch64 host)

unendlich 303K · voiceterm 135K · bo-suite 75K · logscope 50K · snapclone 42K ·
ambient-learner 22K · bot-arena 13K · av1tuner 10K · gitmind 9K · photosync 1.1K ·
rustcheck/peek/drift small. Plus ~92K C++, ~56K QML, ~14K Python.

  **App routing survey (this round) — what the suite wants out of routing:**
  - **bo-suite**: already a real audio MESH (mesh/peer_auth/sync/chan_server) —
    wants to DECOUPLE control from LAN → LAN discovery (mDNS) + beyond-LAN
    control channel, low-latency, peer-auth (has it).
  - **photosync**: self-hosted photo sync SERVER (daemon/sync_server) — wants
    family reach THROUGH NAT → shared-mailbox + content-blob-swarm, **DURABLE**
    (photos persist), private group.
  - **ambient-learner**: weather/screen-temp + DB/history, NO net yet — wants
    community calibration + historical-data SHARING → public/community data pool,
    **DURABLE**, many small contributors, read-mostly.
  - **voiceterm**: zettelkasten ([[zbracket]] .md graph) + security sandbox —
    wants to SHARE note graphs → content-addressed linked notes, **DURABLE**,
    selective (per-note/collection) visibility.
  - **logscope**: LAN LLM serving (SERVING.md, key-gated, instance-stacking) +
    conversation/LIVE sharing — wants live real-time sessions + durable
    conversation artifacts + remote (decouple from LAN), key access.
  **KEY FINDING — DURABLE is the MAJORITY need** (photosync/voiceterm/ambient/
  logscope); only snapclone(snaps)/unendlich(frames) are ephemeral. snapclone's
  "ephemeral by construction" is the minority → the durable content-swarm
  (participant re-replication + opt-in pinning) is FIRST-CLASS, not deferred.
  Requirement clusters: LAN discovery · beyond-LAN NAT reach · durable distributed
  content sharing · public/community pooling · live sessions · access spectrum
  (private/public/key-gated). These JUSTIFY a real distributed DHT — relay-first
  alone would NOT cover durable distributed pooling.
  **DECISION (sequence revised): cleanroom pure-Rust DHT FIRST, then crypto.**
  Scope reframe: "grow microdht's done storage (TTL/re-announce/multi-value/cap)
  into distributed Kademlia + wire existing STUN(done)/UPnP(done)/relay(done) +
  port the hole-punch FSM" — NOT reimplement OpenDHT blind. Kademlia is a
  published algorithm (impl from the paper + microdht semantics), which also moots
  licensing. **Distributed routing IS testable offline** (N nodes on loopback =
  microdht's whole purpose); only NAT-traversal-over-internet is hardware-gated
  (the Fairphones). LICENSE CAVEAT: vendored opendht LICENSE reads MIT-style but
  upstream OpenDHT is GPLv3 — confirm which governs; cleanroom-from-spec sidesteps.

  **Routing-core build + algorithm tweaks (this round):**
  - Phones aren't proxy-relegated by *capability* — modern SoCs run full Kademlia
    fine. The real constraints are network *position* + OS lifecycle:
    OS background-suspension (the big one for STOCK phones — but our fleet is
    mainline-Linux/postmarketOS, so a persistent daemon runs like a server →
    barrier absent), CGNAT inbound reachability, address churn, radio battery.
    => participation is a RUNTIME POLICY, not impossibility (coresched seam).
  - **Algorithm tweaks accepted, all as LAYERS over the k-bucket core:**
    (1) **Explicit clusters** = smaller connected graphs + id-mapping. A cluster
        IS a shared mailbox/group/LAN-affinity set you JOIN (not emergent). Two
        tiers: sparse global Kademlia (find cluster) + dense intra-cluster peer
        set (kept regardless of XOR distance). Node-in-two-mailboxes = live bridge
        = the id-mapping. Unifies with durability-through-population.
    (2) **ID-windowed maintenance scheduling** — batch refresh/re-announce into
        ID-derived time windows to amortize radio TAIL energy; the data-type/
        Lifetime flag trickles app->share-kit->snapcore->coresched as a
        power-policy input (ephemeral never wakes radio; durable earns a window).
    (3) **Policy matrix**: battery/cellular -> minimize radio (ID-window batch);
        wifi/plugged -> minimize CPU via uclamp.max + little-core affinity, be a
        fuller node. Crossed with the data-type flag.
    (4) **HE for "compression" = REFRAME**: HE expands, doesn't compress. The real
        fit is homomorphic AGGREGATION/SKETCHING of bulk events (private
        aggregation): many encrypted readings -> one compact aggregate w/o
        decrypting. Fits ambient-learner community calibration pool (maybe
        logscope analytics). Additive-only (Paillier) or packed lattice; heavy,
        read-mostly pool only, NOT phone hot path. FILED as research spike.
  - **BUILT: `shared/snapcore` v0.1.0** — routing core. NodeId(160-bit)+XOR metric,
    k-buckets with OpenDHT's replacement-cache + good/questionable/bad liveness,
    `closest()` find-node. Zero-dep, logical-clock (deterministic), 9 tests pass
    offline. Layers next: cluster overlay, ID-windowed scheduler, then lift
    microdht storage + wire transport (STUN/UPnP/relay done; hole-punch FSM port).
  - OpenDHT licensing RESOLVED: vendored tree is `SPDX MIT` throughout (LICENSE +
    header tags), so readable for analysis. Real-world necessities it confirms:
    proxy-client/server (= daemon-as-proxy for CGNAT/asleep), multicast LAN
    peer-discovery (= bo-suite LAN path), replacement-cache + liveness, rate_limiter.

  **snapcore iterative lookup + multi-node sim (this round) — DONE:**
  Added store.rs (lifted microdht store: TTL, re-announce dedup+refresh,
  multi-value, per-key cap), node.rs (Node = routing+store, Transport RPC trait,
  FindValue, handlers that learn the caller), lookup.rs (iterative lookup_nodes /
  lookup_value, terminate when k-closest all queried), sim.rs (in-memory Network +
  SimTransport = the net-sim foundation, backlog E1). **19 tests pass offline**,
  incl. the headline: store-on-node-A is findable-from-node-Z purely via routing,
  and lookup converges to the TRUE k-closest after the real Kademlia join
  (each node self-looks-up on bootstrap → star becomes connected). Note: a single
  bootstrap star does NOT converge in one shot (expected Kademlia property) — the
  self-lookup join is what connects it. snapcore now a working DHT in-sim.
  NEXT layers: cluster overlay (explicit shared-mailbox dense peer set) · ID-windowed
  maintenance scheduler + data-type trickle-down · then real UDP transport
  (STUN/UPnP/relay done; hole-punch FSM port) replacing SimTransport.

  **snapcore cluster overlay (resilience layer) — DONE:**
  cluster.rs: explicit clusters (ClusterId = keyspace point, roster lives in DHT
  under that key). Clusters{} = dense per-cluster member sets (no XOR filtering,
  liveness + cap), join/leave/record_member/members/shared_with (the bridge
  relation = id-mapping: a node in 2 clusters bridges them). DHT-backed roster:
  announce() stores contact under cluster key (multi-value), discover() reads it
  back. Resilience primitives: lookup_value_resilient() (ask co-members FIRST,
  global Kademlia fallback) + replicate_to_cluster() (durability-through-
  population). **25 tests pass offline** incl. THE headline: a node with an EMPTY
  global routing table still fetches a value via its cluster co-members
  (cluster_path_finds_value_when_global_path_is_dead), plus announce/discover
  round-trips the roster through the DHT, plus replicate distributes to co-members.
  snapcore now: routing core + value store + iterative lookup + multi-node sim +
  cluster resilience overlay, ~1450 LOC, all offline-provable.
  NEXT: real UDP transport (swap SimTransport; STUN/UPnP/relay done, hole-punch
  FSM the one port — offline-provability hands to field validation) AND/OR
  ID-windowed maintenance scheduler + data-type trickle-down. Then crypto-kit.

  **crypto scan + crypto-kit v0.1 — DONE:**
  Monorepo crypto (by Cargo.toml deps, the reliable signal — usage grep
  over-reports on words like "design"/"verify"): sha2, ed25519-dalek, x25519-dalek,
  hkdf, aes-gcm, fips203 (ML-KEM-768), zeroize, rand_core. snapclone uses the full
  hybrid-PQ stack; other apps a subset (hashing/signing/AEAD). **crates.io IS
  reachable** in this sandbox (online `cargo build` fetches+compiles dalek/fips203/
  aes-gcm) — so crypto crates ARE buildable here (only the pre-seeded offline cache
  lacked them). **shared/crypto-kit v0.1** = snapclone-crypto LIFTED, dual face:
  src/ffi.rs (the verbatim C-ABI sc_* surface for the C++/Qt side, +14 tests) and
  src/lib.rs (a SAFE idiomatic Rust API for Rust consumers: x25519/ed25519/
  ml-kem-768 keygen+agree, derive_hybrid_session_key = HKDF-SHA256(x25519_ss ‖
  mlkem_ss, "snapclone-v1-pq-session"), seal/open AES-256-GCM [nonce|ct+tag]).
  Added Debug/PartialEq derives to CryptoError (ABI-neutral). **20 tests pass**
  incl. full hybrid PQ handshake end-to-end (both sides derive same key; breaks if
  either half breaks) + seal-under-hybrid-key round-trip. snapcore also got
  proximity stubs (BLE/acoustic/adhoc, unimplemented!()+TODO) on the dense layer.
  NEXT: snapclone-group ratchet/seal as crypto-kit's GROUP layer (sender-key
  ratchet + per-recipient seal + epoch), then share-kit (mailbox over snapcore +
  crypto-kit). Note: lifting eventually repoints snapclone-crypto -> crypto-kit
  (other Claude is in snapclone UI; crypto.rs is different files, low collision).

  **crypto-kit GROUP layer — DONE (commit follows):**
  shared/crypto-kit/src/group.rs: sender-key ratchet + per-recipient sealing,
  lifted from snapclone-group/{ratchet.rs,seal.rs}, built ENTIRELY on crypto-kit's
  own primitives (no duplicated AEAD/KDF). SenderChain: from_root/generate(OsRng)/
  next_message_key->(idx,MK)/message_key_for(target)/export_root; CK_{i+1}=
  HKDF(CK_i,RATCHET), MK_i=HKDF(CK_i,MSG_KEY); zeroize on drop+advance; one-way
  (forward secrecy within epoch). seal_sender_key/open_sender_key reimplemented on
  x25519_shared + derive_session_key(SENDERKEY_SEAL) + seal/open (byte-array API,
  OsRng, Option not SealError — consistent w/ crypto-kit). Labels byte-identical to
  snapclone (snapclone-v1-group-*) for wire compat. Content enc = crypto_kit::seal/
  open under MK (no redundant wrapper). **28 crypto-kit tests pass** (+8 group):
  ratchet agreement, forward-secrecy-can't-rewind, distinct-per-index, label
  separation, seal roundtrip, wrong-recipient-fails, and THE headline
  epoch_rekey_excludes_removed_and_newly_added_members (removed member can't read
  new epoch; newly-added can't read prior — proven from ratchet+seal, membership
  bookkeeping deferred to share-kit). crypto-kit now: ffi (C ABI) + safe Rust API
  (hybrid PQ + sign + AEAD) + group (ratchet/seal). ~1.5K LOC.
  NEXT: share-kit — the mailbox over snapcore + crypto-kit. Generalize
  snapclone-group/mailbox.rs's group_id/groupmsg bucket -> app-supplied namespace
  (unendlich sessions + snapclone groups share it); carry Lifetime flag +
  DeliveryHint profiles; wire SenderChain epochs + seal_sender_key fan-out onto
  snapcore's cluster roster (announce co-members = the member set to seal to).

---
## net-kit v0.1 + patches/ workflow  (this session)

[DECISION] DEFER share-kit, build net-kit FIRST. net-kit is the layer *beneath*
share-kit (the mailbox needs transport; transport primitives come first). Good
sequencing: snapcore's future real transport + share-kit's daemon protocol both
layer auth+wire from net-kit.

[WORKFLOW] **patches/** — rendezvous for sister-session output (snapclone-UI port,
logscope distributed-fleet session). Each entry = `<name>.patch` (consolidated
git diff) + `<name>.HANDOFF.md` (spec/API/integration). Two purposes: (1) pending
application to the standalone repo it targets, (2) SOURCE MATERIAL to mine for
shared/ kits. NOT auto-applied (target standalone trees ≠ monorepo snapshot).
First entry: logscope-distributed-fleet (the "speaker reactions in the AI tree"
work — enlist/dispatch/bridge/ollama/localexec/speculate/voicefan/devloop, 117-test
lean suite green; voicefan = speaker→tree fanout BUILT, reaction CLASSIFIER pending).

[SURVEY] Transport/auth/wire/http duplication across the monorepo is severe:
ct_eq in 5+ (voiceterm/mail_crypto, bo-suite/peer_auth, logscope/netserve, both
crypto crates); hand-rolled HTTP/1.1 in ~8 (bo-suite chan/track, logscope
netserve/fabric_net, snapclone-upnp/ssdp); percent line-codec in ~6 (voiceterm
qml_gen/recall/manpages/layout_tool, unendlich/session_wire); JSON parse in
voiceterm/hf_hub + ollama; raw sockets in ~25; FOUR Transport traits (snapcore
node.rs, unendlich sim/net/transport.rs, logscope fabric_net.rs, + enlist).

[CODE] **shared/net-kit** v0.1 — zero-dep (std only), offline-provable, **17 tests
pass --offline**. Standalone crate like the other shared kits (no workspace).
  - auth.rs: ct_eq (constant-time) + generate_key (/dev/urandom + time/pid
    fallback, 64-hex) + Keyring (constant-time membership, empty=open) + frame/
    unframe (token framing "<tok>\t<line>"). = union of netserve auth + enlist auth.
  - wire.rs: esc/unesc (percent %,\t,\n,, — byte-identical to enlist) + encode_
    fields/decode_fields (tab line, flat case). Doc caveat: comma-sub-list fields
    built manually w/ esc (can't compose encode_fields — would double-escape).
  - json.rs: escape + find_string_field (targeted top-level extractor, not fooled
    by key-in-value) + read_json_string (pub; handles \uXXXX). From ollama.rs.
  - http.rs: Request{method,path,host,content_type,body}+post_json()+to_wire()
    (HTTP/1.1, correct Content-Length, Connection:close); Response{status,body};
    parse_response; roundtrip(host,req,timeout) TcpStream seam. From ollama.rs.
[STANCE] net-kit does NOT unify the four Transport traits (different abstractions,
different protocols) — it's the SHARED PLUMBING concrete transports reuse. Trait
unification is a separate design question, parked.

NEXT (net-kit growth, mapped not built): http SERVER side (lift netserve serve/
handle + photosync sync_server); generic UDP transport (abstract enlist's
UdpTransport over encode/decode); then the Transport-trait question as a design
item. THEN share-kit on top of snapcore+crypto-kit+net-kit.

---
## net-kit grows (encode/decode org + http server) + share-kit v0.1  (this session)

[CODE] net-kit additions — still zero-dep, **22 tests --offline**:
  - wire.rs: **Record** {tag, fields} + **to_line/parse** + the **Wire** trait
    (to_record/from_record → encode/decode free). The "basic organization of
    encode/decode": every line message is a TAG + esc'd fields; apps impl two
    record methods instead of hand-rolling a format!/split match (the shape the
    logscope Msg codec writes by hand). wire = TEXT line protocols; binary blobs
    (share-kit) get their own framing.
  - http.rs: SERVER side lifted from netserve — ServerRequest{method,path,query,
    headers,body}+parse/header/token(Bearer|X-Api-Key)/query_param; ServerResponse
    {status,reason,content_type,body}+text/json/to_wire; read_request (blocking
    seam) + serve(listener,handler) accept loop (IO seam). Auth = handler's job via
    auth::Keyring + req.token(). net-kit::http is now client+server.
[DECISION] kept the "net-kit" name (it IS the plumbing crate; no rename). http
lives in net-kit alongside the client. Server is single-threaded (one conn at a
time) — thread at the call site for concurrency; netserve's Arc+thread-per-conn is
the app-level pattern, not baked into the kit.

[CODE] **shared/share-kit** v0.1 — the MAILBOX over snapcore+crypto-kit. Deps:
snapcore(path)+crypto-kit(path)+sha2. **7 tests --offline** (crypto deps cached).
  - keys.rs: content_hash(blob)=hex SHA-256 (64-hex); feed_key(ns,id,epoch)=
    NodeId(SHA-256("{ns}:{id}:{epoch}")[..20]); blob_key(ch)=NodeId(SHA-256(
    "blob:{ch}")[..20]). The "one layer up" hashing snapcore's store comment names.
    BYTE-COMPAT with snapclone "group-feed:<g>:<e>" via namespace="group-feed".
  - mailbox.rs: **Lifetime**{Durable,Ephemeral}+ttl_secs(30d/5m)+replicate();
    **Backend** trait(put/get) + impl for snapcore::Store (local/test backing; the
    networked DHT = production seam, not unit-tested); **Posting**{sender,key_index,
    ciphertext}+to/from_bytes (binary length-prefixed — ciphertext is bytes, NOT
    wire text); **Mailbox**{namespace,id}+post(seal→blob@blob_key + pointer→feed_
    key)+fetch(poll feed→fetch blobs)+fetch_open(resolve MK via keyfn→open).
  - **DHT-AS-HOST TWO-LAYER REALIZED**: per-epoch feed bucket (multi-value
    pointers) + content-addressed blob swarm. Durability = population.
  - HEADLINE TEST post_then_read_recovers_plaintext_for_a_fresh_reader: sender
    export_root@idx0 (the distributed sender key) → next_message_key→(0,mk0) → post;
    a FRESH reader holding only mailbox id + root0 does SenderChain::from_root(root0)
    .message_key_for(0)→mk0 → opens "hello world". Proves feed→blob→open + content-
    addressing + the epoch-root→per-message-key derivation end to end.
  - next_epoch_feed_is_empty_until_posted: epoch in the feed key ⇒ rekey to epoch+1
    starts empty (removed member computing the epoch+1 key string reads nothing).

NEXT (share-kit growth, mapped not built): the NETWORKED Backend (iterative
lookup_value + store-to-closest over a Transport) replacing local-Store; cluster
replication wiring (snapcore::cluster::replicate_to_cluster for Durable = the
network seam); SenderChain epoch mgmt + seal_sender_key fan-out onto the snapcore
cluster roster (announce co-members = seal-to set); a higher Channel/Group over
the mailbox. Then snapclone ADOPTS share-kit (epoch-gated cutover; group-feed
namespace = byte-compat keys).

---
## kitfinder + the next-kit findings  (this session)

[TOOL] **tools/kitfinder** — zero-dep structural clone miner (Algorithm 1, token
level). Lex Rust → extract fn bodies → SEMI-BLIND normalize (blind locals to
V0/V1…; KEEP types/variants/consts (uppercase), call/macro/path segments, field/
method names; literals→STR/NUM/CHR) → intern → (a) exact Type-2 clustering on the
normalized seq, (b) MinHash(32)+LSH(8×4) near-dup ≥0.75, (c) cross-crate mass
aggregation. Crate identity = nearest Cargo.toml. Ran on the whole repo in 10s:
2045 files / ~666K LOC / 82 crates / 25,334 fns ≥28 tok → 887 exact clone classes
(105K dup tokens; 314 cross-crate / 43K tok) + 4010 near edges.

[FINDINGS — ranked]
1. **bo-controller ↔ bo-mux = 27,008 shared tokens** (the single biggest mass):
   estimate_fundamental, lock_phase_to_noise, pull_output, goertzel_mag, drone_/
   passby_canceller — DSP duplicated across the two bo-suite crates.
2. Cross-app AUDIO primitives near-dup: rms/compute_rms (bo-controller, voiceterm,
   snapclone-audio), goertzel (bo-suite), fundamental est. → the primitives under
   #1 also recur in voiceterm + snapclone-audio.
3. CONFIRMED PENDING REPOINTS (tool validates prior lifts): crypto-kit ↔
   snapclone-crypto 4,813 tok; ct_eq across gpu-gemm/voiceterm/**net-kit**; json
   escape across bo-controller/bot-arena/voiceterm/**net-kit**. The kits are right;
   the apps just haven't adopted them.
4. Cross-CUTTING tiny utils ("thousand cuts", broadest reach):
   - TIME/clock: now_unix/unix_now/now_secs/now_ms/now_nanos/epoch — 30+ copies
     across 8+ crates (ambient-learner, bo-controller, gitmind, logscope×N,
     voiceterm, av1tuner, bot-arena, snapclone-face). NOT in any kit yet.
   - ESCAPING: esc/unesc/html/xml/qml/path/json/toml escapers across unendlich,
     voiceterm, bo-suite, bot-arena, snapclone-vcard, net-kit. net-kit already has
     percent+json → EXTEND net-kit + repoint.
   - HEX: reimplemented in netserve, share-kit::keys, crypto-kit, kitfinder.
   - GGUF param-estimate-from-name across bot-arena/logscope/voiceterm (the 3 LLM
     apps); splitmix/lcg RNG (unendlich ×35, also snapcore/kitfinder).

[DECISION — next big kit] **audio-kit** is the recommendation: it captures the
LARGEST single duplication in the repo (27K tok bo-controller↔bo-mux DSP core:
RMS, Goertzel tone detect, fundamental-freq estimation, phase-locked cancellation,
filters) AND extends cross-app (voiceterm separation/rms, snapclone-audio rms). A
coherent, dependency-free DSP-primitives crate; bo-suite's cancellers become
consumers. PARALLEL track = **core-kit** (time + hex + fold escaping into
net-kit) — the broadest-reach thousand-cuts layer. Both are now data-justified,
not guessed. Awaiting greenlight on which to build first.

[META] kitfinder now does for us automatically what the hand-survey did for
net-kit/share-kit — and it re-derived those same signals (ct_eq, json escape,
crypto dup) plus surfaced new ones (audio, time). The doc's Algorithm 2 (call
graph + community detection) + the AI-reviewer layer (name + draft each
extraction) are the future build-out.

---
## audio-kit v0.1 — the kit kitfinder pointed at  (this session)

[CODE] **shared/audio-kit** v0.1 — zero-dep DSP primitives, **25 tests --offline**.
The leaf audio building blocks; apps' strategies (cancellers, diarizers,
synthesis) stay as CONSUMERS. Modules:
  - level.rs: rms (byte-identical lift from podcast_diar/voiceterm-separation/
    snapclone-audio) + peak + lin_to_db/db_to_lin (the spectrogram's 20·log10
    factored out) + normalize_peak.
  - goertzel.rs: goertzel(samples,freq,sr) (NaN/Inf-safe magnitude) + Spectrogram
    Config + spectrogram/spectrogram_at_freqs — LIFTED VERBATIM from bo-suite
    audio_features.rs (the module copied across bo-mux+bo-controller = the 27K-tok
    duplication kitfinder flagged #1). Goertzel over FFT for narrow bands.
  - pitch.rs: estimate_fundamental(samples,lo,hi,sr) — Goertzel bin-scan +
    parabolic interpolation, lifted from passby_canceller. ONE CHANGE: dropped the
    canceller's absolute magnitude floor (tuned for un-normalized levels, wrong
    for [-1,1] audio) → reports the peak whenever energy is found; caller gates on
    rms/voicing. The right split for a primitive.
  - window.rs: Window{Rectangular,Hann,Hamming} coeff/generate/apply (NEW — taper
    for spectral/voice feature work; audio_features uses rectangular).
  - gen.rs: Tone (sine osc, phase accumulator) + Noise (white LCG + Voss-McCartney
    pink) lifted from beeper/masker. Useful for synthesis (unendlich), masking
    (bo), and test signals.
  - HEADLINE TEST generated_tone_is_recovered_by_the_analyzers: gen::Tone(330Hz)
    → goertzel peaks at 330 + estimate_fundamental returns 330±5. The primitives
    compose end to end.

[CONSUMERS] (per user): rusty-board (bo-suite) cancellers/masker/beeper →
goertzel/pitch/gen (or keep specialized cancellers, take primitives); voiceterm/
web2qml separation+features → rms/goertzel/window; logscope voice reactions →
rms/pitch (level/voicing gate on the in-flight generation — ties to the voicefan
reaction classifier in patches/); unendlich game audio → gen.

[VALIDATION LOOP] kitfinder found audio_features as the #1 dup (27K tok, bo-mux↔
bo-controller) → built audio-kit from it → the lifts are byte-faithful so the apps
now MATCH audio-kit (the repoint, like net-kit's ct_eq/json). The tool found the
kit; we built it.

NEXT (audio-kit growth / consumers): repoint bo-suite audio_features → audio-kit
(delete both copies); repoint voiceterm/snapclone-audio rms; wire logscope voice-
reaction level/voicing gate. SEPARATE: the bo-internal cancellers (drone/passby,
~27K beyond audio_features) are a bo-dsp split WITHIN bo-suite (strategies, not
cross-cutting primitives) — own crate or shared module, app-local. Possible
audio-kit additions if they recur: zero-crossing rate (podcast_diar has it, a VAD
feature), a generalized harmonic-ratio (bo's engine_timbre_score, pure-on-goertzel
→ voicing detection), biquad filters.

---
## CORRECTION + rusty-board ingested  (this session)

[FIX] I conflated two apps. **bo-suite = the SPEAKER-CONTROL app** (the cancellers/
masker/beeper/Goertzel = audio-kit consumers). **rusty-board = a SEPARATE app: a
GTK4/libadwaita on-screen VOICE KEYBOARD** (Gboard-style swipe typing + whisper-rs
voice input, Wayland layer-shell, D-Bus). Earlier audio-kit notes/commits that say
"rusty-board (bo-suite)" mean bo-suite. audio-kit lib.rs doc fixed.

[INGEST] **apps/rusty-board** added to the monorepo (was standalone, not in the
tree). 54 rs files. Structure: gestures/ (the swipe engine — adaptive_dictionary,
ai_swipe, boundary, cardinal 49K, hitbox_nudger, llm_swipe, ngram, recognizer 41K,
swipe 45K — speed-aware path→word decoding, Damerau-Levenshtein, n-grams, velocity
pivots), voice/ (whisper: recorder/transcriber/service/hallucination/readiness),
plus ui/ keyboard/ layouts/ steno/ llm/ ipc/(D-Bus) compositor/ config/. GTK4 deps
→ won't build in sandbox (system libs); ingest+survey only.

[AUDIO-KIT CONSUMER] voice/recorder.rs `current_level()`/`level_meter()` =
mic-level metering (audio-kit::level). VAD/voicing gating on whisper input →
level::rms + pitch. Modest but real — rusty-board joins the audio-kit consumer set
(voice-input side, distinct from bo-suite's speaker-output DSP).

[FLAG] rusty-board's swipe gestures are BROKEN (per user). The engine is large
(~100K of gestures/) and speed-aware (stop-velocity thresholds, velocity-minima
pivots = deliberate letter stops). Symptom not yet specified — need the failure
mode before diving. Candidate future: a voice-kit shared by rusty-board + voiceterm
(both whisper voice apps).

---
## voice-kit v0.1 + kitfinder bundle export + swipe breadcrumb  (this session)

[CODE] **shared/voice-kit** v0.1 — zero-dep whisper-voice TEXT+MODEL layer, **10
tests --offline**. The pure logic rusty-board (voice keyboard) + voiceterm/web2qml
both apply AROUND whisper. Sits beside audio-kit (the SIGNAL layer); whisper-rs
binding + mic capture stay in the apps.
  - hallucination.rs = the UNION (rusty-board's hallucination.rs even credits
    voiceterm's filler_words.rs): is_hallucination (canonical-list exact match) +
    strip_trailing_artifacts ([BLANK_AUDIO]-off-the-end) from RUSTY-BOARD; +
    trim_hallucination_loop (repeated-shingle collapse) + looks_like_speech
    (length/repetition/bad-phrase gate) from VOICETERM. Merged lists. NOTE: the
    lifted loop-trimmer has a faithful quirk — exclusive bound `0..(len-window*3)`
    means a shingle repeating exactly 3× and filling the whole string isn't caught
    (needs one extra token); kept as-is (lift fidelity), tests match.
  - model.rs = ggml resolution generalized from rusty-board model_path.rs: MODEL_
    CANDIDATES (quantized .gguf > .bin > fp16, base>tiny) + find_model_in(dirs,env)
    (app supplies dirs) + Variant::from_filename (size/.en/quant parse).
[DECISION] voice-kit is ZERO-DEP for v0.1 (text+fs only). A future VAD gate is the
one place it'd gain an audio-kit dep (rms threshold + hangover).
[CONSUMERS] rusty-board src/voice/ + voiceterm src/voice.rs (both working great in
latest per user) → repoint to voice-kit. Future: VAD gate over audio-kit::level.

[EXPORT] **kitfinder.bundle** built (standalone git bundle: Cargo.toml + main.rs +
README incl. a "hunt a multi-world bug" section) → handed to the user to pass to
the sister unendlich conversation. The README explains: run kitfinder on unendlich,
near-dup families surface a diverged copy of per-world logic (the bug often lives
in the 0.8–0.95-similar odd-one-out). Presented via outputs.

[BREADCRUMB] patches/rusty-board-swipe-regression.NOTES.md — swipe worked in an
EARLY iteration (strong core), broke from perf+leniency+features, never recovered.
Good core believed between commits 876a0fa..7d3efea (rusty-board-git.zip uploaded;
apps/rusty-board = latest broken HEAD; the 17 range-commits don't touch swipe.rs/
recognizer.rs by name → inspect the whole gestures/ at the endpoints). FUTURE:
recover core via bisect/diff; reusable primitive = a swipe/touch-kit (path→keys→
words). NOT active now.

---
## core-kit v0.1 + first kit-to-kit repoint  (this session)

[CODE] **shared/core-kit** v0.1 — zero-dep foundational leaf utilities, **12 tests
--offline**. The BROADEST-reach duplication (most places, not biggest clones), per
kitfinder: the LCG multiplier 1664525 appears 137×, splitmix constants dozens,
now_secs/now_ms/now_nanos copied across 8+ crates, hex {:02x} loops in 4+.
  - time.rs: now_secs/now_millis/now_nanos (SystemTime→UNIX_EPOCH, .unwrap_or(0)
    clock-skew-safe; nanos is u128) + Stopwatch (monotonic Instant wrapper).
  - hex.rs: encode (lowercase, table-driven) + decode (upper/lower, None on
    odd-len/non-hex). The canonical {:02x}/from_str_radix replacement.
  - rng.rs: SplitMix64 (the constants kitfinder/audio-kit/snapcore-sim all copy) —
    next_u64/next_f64/next_bounded/shuffle. NON-crypto (keys → crypto-kit/net-kit).
[STANCE] core-kit is the BOTTOM of the kit stack — pure, zero-dep, depended on even
by other kits. rng adoption in EXISTING code is opt-in (swapping a PRNG changes its
sequence; not a blind find-replace) — documented.

[REPOINT — VERIFIED] Dogfooded: share-kit::keys dropped its private `hex` fn and now
calls core_kit::hex::encode (added core-kit path dep). share-kit rebuilds, 7/7 still
pass. FIRST cross-kit dependency — the stack is now a real DAG (share-kit →
{snapcore, crypto-kit, core-kit}). Demonstrates the repoint pattern on code that
actually builds in the sandbox (unlike the stale apps).

[REPOINTED — all verified] net-kit::auth hex → core_kit::hex (22 tests pass);
audio-kit::gen Noise LCG → core_kit::rng::SplitMix64 (25 tests pass; noise quality
improved, API unchanged, tests check properties not exact values). So THREE kits now
build on core-kit (share/net/audio) — every kit-internal hex/rng copy is gone; the
remaining copies live only in the (un-sandbox-buildable) apps. kitfinder keeps its
inline hex/splitmix on purpose (stays portable/standalone for export bundles).

[KIT STACK now] core-kit (foundation) · registry-kit · snapcore · crypto-kit ·
net-kit · share-kit(→core) · audio-kit · voice-kit. 8 kits + kitfinder.

NEXT candidates (open): the ESCAPING family is heterogeneous (percent=transport/
net-kit; html/xml/toml/path=general text) → needs a where-does-it-live decision,
not a quick build. App repoints are the remaining payoff but un-verifiable in
sandbox (stale apps, heavy deps). voiceterm loop-trimmer off-by-one (found earlier)
is a fix-at-source-when-repointing item.

---
## App→kit repoints — first apps onto the kits  (this session)

Synced all apps to sister-latest (commit 1), then repointed the clean kit
consumers (commit 2). The 5 repoint-target files were byte-identical sister↔
monorepo (the sister conversations worked elsewhere — spatializers, swipe, biomes),
so the repoints are correct against what the user builds.

[REPOINTS — verified via standalone harness crates, `cargo check` green vs the kits]
  - bo-suite `audio_features.rs` (bo-mux + bo-controller, byte-identical) → SHIM:
    `pub use audio_kit::goertzel::{spectrogram, spectrogram_at_freqs, SpectrogramConfig};`
    + `engine_timbre_score` kept local (built programmatically from the existing fn,
    `use audio_kit::goertzel::goertzel as goertzel_mag;` injected → zero body changes).
    This is the headline: the 27K-token audio_features duplicate audio-kit absorbed.
  - bo-controller `podcast_diar.rs` rms + voiceterm `separation.rs` compute_rms →
    1-line wrappers over `audio_kit::level::rms` (name kept, call sites unchanged).
  - rusty-board `voice/hallucination.rs` → SHIM:
    `pub use voice_kit::hallucination::{is_hallucination, strip_trailing_artifacts};`
    (callers in ui/window.rs unchanged; voice-kit's fns ARE rusty-board's, behavior-preserving).
  - Cargo deps added: audio-kit to bo-mux/bo-controller/voiceterm; voice-kit to rusty-board.

[CAN'T verify in-sandbox] full app builds (Qt/GTK). Harness crates prove the kit-
integration surface compiles; user builds+runs the apps via GUI to confirm the rest.

[FOLLOW-UP repoints] voiceterm `voice.rs` hallucination fns (embedded in the big
file, not a module → place to fix the loop-trimmer off-by-one at source); unendlich
worldgen RNG → core_kit::rng (opt-in, changes sequence); the scattered now_ms/hex
app copies → core-kit. Plus the only-in-monorepo reconcile (SISTER_STATUS.md).

---
## Build / deploy phase — kickoff  (this session)

Moving from kits → builds/integration/deploy. Existing infra found (good shape):
build/build.sh (manifest-driven cross-target orchestrator, plan-or-run),
build/flatpak.sh (flatpak plan stub), verify.sh (rustcheck + coarse wiring
suspenders), manifest/{projects,targets}.toml.

[FIXED] projects.toml only had registry-kit under [shared.*] — registered the other
7 kits (core/snapcore/crypto/net/share/audio/voice), all kind=cargo role=shared-lib,
targets desktop/phone/x86/alpine (pure Rust → cross-clean). Verified core/net/audio/
voice build through `build/build.sh --run` on the host (sandbox host = x86_64-gnu, so
they build for the x86 target here; on the user's Asahi box they'll build desktop).

[NEW] build/deploy.sh + manifest/fleet.toml — deploy phone builds to the FP5 fleet
VIA GITMIND. Model: gitmind is gate + distribution. `gin check` records green per
commit; deploy ships a green commit; device activates via `gin app build|install
--commit C --target phone --use --desktop`. Two per-app modes: build (phone builds
natively, aarch64, no cross sysroot) | pin (desktop cross-builds aarch64-musl, gin
app install pins the binary). Plan-printing stub (consistent with build/flatpak);
real run needs fleet SSH + a green commit. fleet.toml is a template (placeholder
device addresses).

[COMPLETENESS] Sync (55fa049) captured ALL sister work incl. GUI: bo-suite controller
QML, gitmind Apps.qml + peek crate, logscope anamorph prototypes, unendlich shaders +
WorldSession, voiceterm mail, snapclone C++ mid-call/tiling UI. NOT in this hand-off:
ambient-learner, av1tuner, bot-arena, photosync (sit at prior state).

[RECONCILE] gitmind's synced crates/peek == canonical tools/peek (v0.2.0, byte-id) →
gitmind should path-dep tools/peek instead of carrying a copy (dedup follow-up).

NEXT (build/integration/deploy roadmap):
- cross sysroots: `rustup target add aarch64-unknown-linux-musl` + musl linker for the
  phone target; then build/build.sh --run can actually cross-build (currently plans).
- flatpak.sh: resolve runtime=tbd (av1tuner, gitmind), wire flatpak-cargo-generator.
- deploy.sh --run once fleet addresses + mode decided per app.
- follow-up repoints (voiceterm voice.rs hallucination; unendlich RNG; now_ms/hex sweep).

---
## Wire voiceterm onto kits + fix loop-trimmer at source  (this session)

[FIX] voice-kit trim_hallucination_loop had an EXCLUSIVE loop bound
0..(len - window*3) → a shingle repeating exactly 3× and filling the whole string
(len == window*3 → 0..0 empty) was never caught. Fixed to inclusive (0..=) + added
an exact-fill test ("go left go left go left" → "go left"). voice-kit 10 tests green.
This is the latent voiceterm bug, fixed at SOURCE so every consumer gets it.

[REPOINT] voiceterm fully onto kits: trim_hallucination_loop → voice-kit (inherits
the fix); ALL ~19 named time helpers (now_ms/now_secs/epoch_secs + i64 variants
across acoustic_index/audit_log/decision_cache/llm_cache/log/log_summary/mail/
mail_drafts/mail_session/missing_packages/phonetic/scroll_signals/system_info/
term_blocks/token_stats/util/web_summary/workspaces/voice) → core_kit::time wrappers.
Added core-kit + voice-kit deps; cleaned dead `use std::time::{SystemTime,UNIX_EPOCH}`.
20 files, net -97 lines. Verified via harness crate (wrapper shapes cargo-check clean
vs core-kit + voice-kit); voiceterm itself is Qt-blocked in-sandbox. Inline non-fn
time uses in audit_log:617 / phonetic:84 left (not extractable as simple wrappers).

---
## GUI deploy cockpit  (this session)

Goal (user): build the gin GUI on laptop + phone, and from the GUI discover SSH
devices → push the monorepo to a picked folder → get a .desktop launcher on
Plasma Mobile. The GUI already had the back half (app_install/app_desktop via
gitmind_core::binpin::write_desktop); added the front half.

[NEW] apps/gitmind/crates/gui/src/deploy.rs — pure std (no Qt), 7 unit tests:
local_subnet24 (UDP-route trick), scan_ssh (threaded /24 port-22), list_remote_dirs
(ssh cd+pwd+ls, returns absolute pwd + subdirs), push_repo (rsync -az --exclude
target/), remote_deploy_cmd/deploy_app (ssh: cargo build → gin import → gin app
install --use --desktop). Command builders split out so they're asserted without
network.
[WIRED] Backend gains 4 qt_methods (discover_devices/list_remote_dirs/push_repo/
deploy_app) wrapping deploy::*; Deploy.qml is the touch UI (discover → device list
→ folder picker → push + build/launcher); registered in qrc!, added to main.qml
SwipeView + tab bar. gui crate rustcheck CLEAN.
[CAN'T VERIFY HERE] the Qt build (no Qt6 in sandbox) + the actual ssh/rsync/scan
I/O (no phone/network). Logic is unit-tested; Qt wiring follows the existing
Backend pattern exactly. User builds `cargo build -p gitmind-gui` on-device.
[NOTE] discover/push/deploy are synchronous → the UI blocks during them (scan
~1-2s, build longer). Async (qt_signal from a worker thread) is a future polish.

---
## Kit-adoption sweep (this session)

[voiceterm] voice.rs is_sane_transcription → voice_kit::hallucination::looks_like_speech.
voice-kit's BAD_PHRASES is byte-identical to voiceterm's inline list and the length (2..=120)
+ >50% repetition rules match → EXACT behavior (verified on 8 branch-boundary cases). voiceterm
rustcheck clean. Completes voiceterm's adoption of voice-kit's hallucination module.

[core-kit::time sweep across 5 apps] Repointed 11 genuine epoch duplicates (the SystemTime::now()
.duration_since(UNIX_EPOCH)…unwrap_or(0) pattern) to core_kit::time wrappers:
  rusty-board ×4 (adaptive_dictionary, ngram, readiness, service)
  bo-suite ×2 (scanner, library) · logscope ×2 (chatdump, providers)
  av1tuner ×1 (job) · ambient-learner ×2 (screen_temp_hints, pin_store)
i64 callers cast (core_kit::time::now_secs() as i64); now_ms bridged to now_millis. 6 crates
gained the core-kit dep (depth-correct paths ../../ … ../../../../). 4 orphaned SystemTime/
UNIX_EPOCH imports removed (scanner/library keep theirs — still used elsewhere).
CRITICAL — 2 MONOTONIC helpers deliberately LEFT ALONE: rusty-board swipe.rs now_ms()
(process_start().elapsed()) and bo-suite bt_respect.rs now_secs(&self) (session_start.elapsed())
are Instant-based, not epoch — routing them to the wall clock would silently change meaning.
All 5 apps rustcheck clean; wrapper targets verified against core-kit.

[NOT swept] hex dup in bo-suite (1 file) — minor, deferred. peek dedup (gitmind/crates/peek ==
tools/peek) left intentionally: deduping breaks apps-are-standalone.

---
## GUI deploy cockpit — async polish (this session)

discover/push/deploy were synchronous → the GUI froze for their duration (scan
1-2s, remote build longer). Made them async, worker-thread + poll:
[deploy.rs] added Job (Mutex<JobInner>: running/kind/done/result) with start()
(claims it; refuses a 2nd concurrent job), finish() (worker reports JSON result),
poll() (GUI reads; consumes the done result so it's delivered exactly once). New
8th unit test drives the full claim → off-thread finish → poll-once → idle cycle.
[main.rs] discover_devices/push_repo/deploy_app → start_discover/start_push/
start_deploy: each claims the Job, spawns a worker (captures Arc<Job> + owned
Strings, never self), returns immediately. New poll_job() qt_method; new field
job: Arc<deploy::Job>. list_remote_dirs stays sync (one quick ssh ls).
[Deploy.qml] start_* + a 250ms repeat Timer (poller) that polls poll_job and, on
done, dispatches via applyJobResult (renamed from onJobDone to dodge QML's
on<Signal> handler-naming rule). Added an animated BusyIndicator (works now the
UI thread is free); action buttons stay disabled while busy.
gui crate rustcheck CLEAN; QML balanced 58/58·60/60; deploy module 8/8.
[edge, noted] a panicking worker would leave the Job running=true (UI stuck busy)
— the ops are I/O-returns-Result so low risk; wrap in catch_unwind if it shows up.

---
## GUI file browser — the Files tab (this session)

User built gitmind-gui on-device; on a clean repo the GUI showed nothing
browsable (status="clean", and the only file-view path was peeking a *changed*
file — none exist when clean). The "view" tab could render a file but had no
listing to pick from. Added a real browser:
[browse.rs] list_repo_dir(root, subpath) — lists one working-tree dir, skips
.git/target/node_modules, sorts, refuses path escapes (canonicalize + starts_with).
1 unit test (sort/skip/descend/escape).
[main.rs] mod browse; new list_dir qt_method wrapping it; Files.qml in qrc.
[Files.qml] ONE tab that browses (tap folder=descend, ‹..=up) and views (tap
file → peek_file with the spec field; ‹back returns), toggled by selectedFile.
Replaced the bare PeekViewer tab AT THE SAME INDEX (3) so nothing else shifts;
status panel's peek action now routes to files.openPath(). "view" tab → "files".
PeekViewer.qml left in tree but no longer instantiated (folded into Files).
Empty clean-status now signposts the Files + Timeline tabs.
gui crate rustcheck CLEAN; browse+deploy modules = 9 tests. Qt build on-device.

---
## GUI network lookup + install — the Network tab (this session)

User: "add network lookup and installing to the GUI." Used the pieces gitmind
already had: announce::ProjectCard (content-addressed payload: name, refs, ssh
provider, name_key) + net-kit::http (serve + roundtrip) — the transport the
announce.rs comment anticipated ("DHT/hosting later"). net-kit is pure-std (core-
kit only); added as a gui dep (../../../../shared/net-kit).
[netshare.rs] Advertiser (stoppable serve state: stop AtomicBool + addr Mutex);
card_for_repo(root,user,host) → card with repo name + git for-each-ref refs +
ssh://user@host<abs> provider; serve_loop (non-blocking accept of GET /card until
should_stop); lookup_peer(addr) → GET+parse a peer's card; install_card → git
clone the ssh provider. 4 unit tests (card build, GET request, install validation,
advertiser state) against the REAL announce + net-kit.
[main.rs] net-kit dep; advertiser: Arc<netshare::Advertiser> field; advertise_
start/stop/status + lookup_peer (sync, 4s) + start_install (async — reuses the
deploy worker Job + poll_job). bind() happens synchronously in advertise_start so
port-in-use errors surface; the serve thread runs serve_loop then adv.end().
[Network.qml] advertise toggle (shows serving host:port), look up a peer by addr →
renders the card (name/desc/refs/provider), install → start_install + the 250ms
poller (busy spinner). 7th tab.
gui crate rustcheck CLEAN; gui logic = 13 tests (deploy 8 + browse 1 + netshare 4).
Live serve/GET/clone = on-device. [note] 7 tabs now — could fold deploy+network
into a "fleet" tab if crowded.

── ssh: non-interactive + host-key trust through the GUI (fixes live-on-device freeze) ──
PROBLEM (GUI live on phone+workstation): user="user" assumption + first-connect host-key
prompt made `ssh user@host` block on input the GUI has no TTY for → worker/UI froze; and
accepting keys in the terminal vs GUI split known_hosts (locks).
[deploy.rs] SSH_OPTS = [-o BatchMode=yes -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new]
applied to EVERY ssh/rsync/git-ssh call: list_remote_argv (via ssh_argv helper), push mkdir
(ssh_argv), rsync (-e ssh_command_str()), deploy_app (ssh_argv). BatchMode = no password hang;
ConnectTimeout = no unreachable hang; accept-new = auto-trust NEW host keys into the shared
~/.ssh/known_hosts (still rejects CHANGED keys) → GUI manages trust, no terminal split.
+ host_trusted(host)->bool (ssh-keygen -F, fast/local) and trust_host(host)->Result<fp> (ssh-keyscan
-T 6 → append to known_hosts; pure helpers filter_scanned_keys/parse_fingerprint unit-tested).
[netshare.rs] install_card git clone got .env("GIT_SSH_COMMAND", same opts inline) — no
cross-ref to deploy::SSH_OPTS (keeps the standalone test harness independent).
[main.rs] current_user()->{user} (whoami), host_trusted(host)->{trusted}, start_trust(host) (async
via the SHARED Job, kind "trust" — keyscan does I/O so off-thread; poll_job delivers it).
[Deploy.qml] Component.onCompleted defaults root.user from backend.current_user(); device section
shows a 🔒trusted / "key not trusted" badge + a "trust key" button (start_trust + the existing
250ms poller; applyJobResult handles kind "trust" → refreshTrust()). Network.qml needs no user
field (install uses the provider URL's embedded whoami; clone covered by accept-new).
SANDBOX has NO ssh/ssh-keygen/ssh-keyscan/rsync (only git) → spawn-failure path returns false/err
(no panic); real ssh I/O is on-device. deploy 10 tests (was 8: +scan-filter/fingerprint, +unknown-
host-not-trusted) PASS; netshare 4 PASS; gui rustcheck CLEAN; Deploy.qml balanced. gui logic = 15 tests.

── ssh edge cases through the GUI: async browse + classified errors + key-setup wizard ──
SYMPTOMS (GUI live, browsing the *advertiser* from the other end): CursorHandle.qml:65
"target undefined" typeerror on tapping browse + no password/file UI. ROOT: list_remote_dirs
was the one op still SYNCHRONOUS → tapping browse froze the UI thread mid-touch (up to
ConnectTimeout) → Qt's touch cursor-handle tore down (that typeerror); and with BatchMode=yes
ssh now *fails fast* on missing key auth between the two devices, but the GUI swallowed the
error instead of saying "auth".
[deploy.rs] ssh_error_reason(stderr)->auth|hostkey|unreachable|other (pure, tested). untrust_host
(ssh-keygen -R) + retrust_host = untrust+trust (clears a stale/changed key — the accept-new case
that WON'T auto-resolve). ensure_local_pubkey (reuse id_ed25519/rsa/ecdsa or generate ed25519
-N "" -C gitmind-fleet). setup_key(user,host,password): ensure key → install pubkey into remote
authorized_keys via PASSWORD auth fed through SSH_ASKPASS (temp 0700 helper echoing $GM_SSH_PW,
SSH_ASKPASS_REQUIRE=force; GUI has no TTY so ssh uses it) → passwordless after. setup_key_argv
forces PubkeyAuthentication=no + PreferredAuthentications=password + accept-new (pure, tested).
ASKPASS_BODY const tested. Password NEVER written to disk (env on the child only; helper just
echoes it); cleared from QML on success.
[main.rs] start_browse (async; result {ok,path,dirs} or {ok:false,error,reason}), start_retrust,
start_setup_key — all via the shared Job + poll_job.
[Deploy.qml] browse() now async (kills the freeze→handle-teardown). applyJobResult handles
browse/retrust/setupkey; on browse failure sets lastReason + a human hint() and auto-reveals the
setup row when reason=auth. AUTO-RETRY browse after a successful trust/retrust/setupkey. Connection
block: trust / re-trust / "set up SSH key" buttons + a password field (echo Password) → install_key.
selectDevice resets curPath/dirs/lastReason/sshpass/showSetup.
SANDBOX has no ssh/keygen/keyscan → setup_key/retrust I/O is on-device; pure logic (reason class,
argv, askpass body, no-network guards) IS tested: deploy 12 tests PASS (+ssh_errors_classify,
+key_setup_argv), netshare 4, gui rustcheck CLEAN, Deploy.qml balanced. gui logic = 17 tests.
NOTE: CursorHandle.qml is a Qt-internal touch artifact (can't patch); async browse stops it firing
on browse; any residual is benign console noise.

── two-sided fingerprint matching + bluetooth send + transport seam ──
USER asks: (1) on-screen two-sided public-key matching [BUILD now], (2) bluetooth file
transfer [BUILD, on-device], (3) eventual snapclone/DHT device IDs + sharekit wrapping
gitmind code [DESIGN seam + note, not built].
[deploy.rs] refactored trust_host → fetch_peer_key (scan, no write) + write_known_host
(append given bytes). NEW for matching: parse_key_summary (pure: "256 SHA256:x c (ED25519)"
→ (fp,type)), key_fingerprint_art (ssh-keygen -lv - → fp+type+RANDOMART; on-device),
local_host_pubkey (/etc/ssh/ssh_host_*_key.pub), local_identity_pubkey (~/.ssh/id_*.pub).
[bttransfer.rs NEW] parse_bt_devices (bluetoothctl "Device AA:BB.. name", skips Controller/junk,
6-byte addr check), bt_push_command(tool,addr,file) for bt-obex|obexftp|bluetooth-sendto (pure),
BT_TOOLS, bt_devices/detect_bt_tool/bt_send (on-device). 3 tests.
[netshare.rs] install_card now SCHEME-AWARE: ssh:// wired; bt://, dht:// → clear "not wired yet"
(transport seam). 
[main.rs] +mod bttransfer. Methods: local_fingerprint()->{host,identity:{present,fingerprint,
type,art}} (sync, local), start_preview(host) (async; {ok,fingerprint,type,art,keys}),
trust_previewed_key(host,keys) (sync write of EXACT compared bytes), bluetooth_devices(),
start_bluetooth_send(addr,file) (async, kind btsend), make_bundle(dest) (git bundle create --all
→ path; default /tmp).
[Deploy.qml] "verify key" btn → start_preview → preview panel (Rectangle, monospace fp+randomart)
→ "✓ matches — trust" calls trust_previewed_key then browse(""). applyJobResult +preview branch
(WATCH: had a missing `}` closing the preview else-if before setupkey — fixed; QML now {107}=).
selectDevice resets preview state.
[Network.qml] "this device's key" block (local_fingerprint on load → fp+randomart, advertiser side
of matching). "send over Bluetooth" block: refresh devices → tappable list → file field +
"bundle repo" (make_bundle) + send (start_bluetooth_send). +btsend in applyJobResult.
[maintenance/SHARING.md NEW] transport roadmap: provider schemes (ssh wired, bt/dht reserved),
what exists now, where sharekit fits (extract netshare ProjectCard+transport into shared/sharekit,
opendht device IDs → dht://, snapclone transport, bt:// provider), verification reality.
HOW MATCHING WORKS: device A Network tab shows "this device's key" SHA256+randomart; device B
Deploy tab "verify key" fetches A's key, shows received SHA256+randomart; user compares both
screens; if match → "matches — trust" writes exactly those bytes. Closes the TOFU/MITM gap accept-new
leaves. SANDBOX has no ssh/bluetooth/radio → keyscan/keygen/OBEX I/O on-device; pure logic tested:
deploy 12, netshare 4, bttransfer 3, gui rustcheck CLEAN, both QML balanced. gui logic = 19 tests.

── defense-in-depth ssh + confirm dialog + app-catalog foundation ──
THREE asks. (1) defense-in-depth so a compromised device can't tunnel into all trusted
devices; (2) push+launch confirm dialog showing full path (was truncated on mobile);
(3) redesign push+launch → pull-based app distributor (broken now). Built 1+2 fully, built
3's verifiable FOUNDATION + designed the build-ship pipeline.
[deploy.rs] SSH_OPTS += ForwardAgent=no, ClearAllForwardings=yes, ControlMaster=no (outbound
anti-pivot — stops agent hijack/tunnels when WE connect to a maybe-compromised peer). NEW pure:
lan_cidr_from_ip (192.168.1.7→192.168.1.0/24; hostname→None), authorized_keys_options(host)
→ "restrict,from=\"<cidr>\"" (IP) or "restrict" (hostname); restricted_key_line(opts,pubkey).
setup_key now installs the RESTRICTED line (restrict = no port/agent/X11 fwd, no pty; from= pins
to LAN /24 so a stolen key won't work off-LAN). +remote_capture(user,host,cmd) generic ssh stdout
(keeps deploy.rs std-only; main.rs glues it to appcatalog). deploy 14 tests (+2).
[appcatalog.rs NEW] recognized-repo scan + metadata. AppEntry{name,is_cargo,is_git}.is_app();
AppDetails{crate_count,size_human,readme_excerpt}. remote_listing_cmd (per-subdir "<flags>\tname",
c=Cargo.toml g=.git) + parse_app_listing; remote_details_cmd (CRATES:/SIZE:/READMESTART..END) +
parse_app_details; LOCAL scan_dir + app_info_local (tested w/ temp dir); human_size. SEAM (tested,
NOT wired): cross_build_argv(app,triple)=["cross","build","--release","--target",t,"-p",app],
install_dir(home)=~/.local/bin, built_binary_path, BUILD_TARGETS[aarch64 musl/gnu]. 6 tests.
[main.rs] +mod appcatalog. start_browse now returns {entries:[{name,app,cargo,git}]} (was dirs)
via remote_capture+remote_listing_cmd+parse_app_listing. +app_details(user,host,path) sync ssh →
{crate_count,size,readme}.
[Deploy.qml] dirs→entries; listing badges recognized apps (📦) + "app info" pill → showAppDetails
→ metadata panel (name/crates/size/README). push+launch buttons → askConfirm → Dialog (centered,
modal, monospace WrapAnywhere full path — fixes mobile truncation) → doConfirm → push/deploy.
selectDevice resets entries/appPath/showAppInfo. Deploy.qml {129}= balanced, all backend.* resolve.
[maintenance/DISTRIBUTION.md NEW] the pull-based distributor flow (browse recognized apps → app info
→ pick build target → server cross-builds → ship binary to ~/.local/bin or /tmp+cp), what's built
(catalog+metadata+seam, tested) vs on-device next (build-type picker, cross build, scp binary,
dependency resolution = "gitmind package manager"). ties to sharekit/dht.
SANDBOX: ssh/cross/radio absent → key-install/cross-build/scp on-device; pure logic tested:
deploy 14, appcatalog 6, netshare 4, bt 3, rustcheck CLEAN, both QML balanced. gui logic = 27 tests.

── build-flag customization + wired cross-build→ship (/tmp staging) ──
USER (confirming pipeline direction): (A) scan default build flags + enable customization
via CMake options + cargo feature scanning, expose to client with repo defaults
PER-ARCHITECTURE enabled; (B) delivery = stage via /tmp (errors easier) else direct.
[buildflags.rs NEW] scan_cargo_features(toml)→CargoFeatures{all,default} (hand-parsed [features],
single-line). read_section_array + arch_default_features(toml,triple): per-arch convention
[package.metadata.gitmind] default-features-musl/-gnu = [...] (matched by triple libc substring)
ELSE [features] default. scan_cmake_options(text)→Vec<CmakeOption{name,default_on,help}> (option(
NAME "help" ON/OFF)). cargo_build_argv(app,triple,features,no_default) (cross build + --features/
--no-default-features). cmake_configure_argv(build_dir,opts,toolchain) (-D + toolchain seam).
built_binary_path/install_dir (MOVED here from appcatalog). BUILD_TARGETS[aarch64 musl/gnu].
6 tests. (appcatalog seam removed → 5 tests.)
[deploy.rs] scp_pull_argv(user,host,remote,local) reuses SSH_OPTS (same anti-pivot) + scp_pull
(on-device). deploy 14 (scp assertion folded into outbound-ssh test).
[main.rs] +mod buildflags. app_build_options(user,host,path): cat Cargo.toml+CMakeLists over ssh →
{targets,features,defaults_by_target:{triple:[feat]},cmake_options,is_cargo,is_cmake}. install_app(
user,host,path,app,triple,features,no_default,stage_tmp) ASYNC job: (1) remote_capture "cd path &&
cross build … -p app" (build errors returned to client), (2) scp_pull binary→/tmp/<app> (or direct),
(3) std::fs::copy /tmp→~/.local/bin + set_executable. stage_tmp default true (/tmp copy stays for
debugging). +set_executable free fn (0o755).
[Deploy.qml] installer configurator (instOpen): openInstaller→app_build_options→populate; target
picker (musl/gnu pills) → applyDefaultsForTarget re-ticks per-arch defaults (reassigns instFeatures
array so Repeater rebuilds); cargo feature CheckBoxes (pre-ticked per arch); cmake option CheckBoxes
(exposed; build needs toolchain, yellow note if pure-cmake → install disabled); stage-via-/tmp toggle
+ live "→ ~/.local/bin/<app> (via /tmp/…)" dest; "download + install" → runInstall (no_default=
features.length>0, ticked set authoritative) → install_app job. applyJobResult +install branch.
"build + install…" button added to app-info panel. Deploy.qml {175}= balanced, all backend.* resolve.
[DISTRIBUTION.md] updated: flags scanning + per-arch convention + /tmp staging (default) vs direct +
cargo path WIRED; remaining on-device = cmake cross-build (toolchain file) + dependency resolution.
SANDBOX: no cross/ssh/scp → cross-build+scp+cp on-device; pure logic tested: deploy 14, buildflags 6,
appcatalog 5, netshare 4, bttransfer 3 = 32 gui tests; rustcheck CLEAN; Deploy.qml balanced.
HOW PER-ARCH DEFAULTS WORK: app_build_options returns defaults_by_target; QML pre-ticks the matching
target's set; user toggles; build sends exactly the ticked features with --no-default-features.

── one connection UI + Deploy|Install toggle + managed fleet user ──
USER insight: the Deploy tab conflated two opposite-direction use cases — push
(desktop→device) and fetch/install (phone←desktop). Want ONE shared connection UI
with a Deploy|Install toggle (not two functions mashed together / not an 8th tab).
Also: stop defaulting the ssh user to whoami(kesslerd); use a GUI-managed dedicated
fleet user/group if that's good linux practice (it is — service account).
[DESIGN] Deploy.qml → Fleet.qml (qrc + main.qml: tab "deploy"→"fleet", Fleet{id:fleet}).
The file already held BOTH flows (push controls + the install configurator from last
turn) — so the fix is a `mode` toggle ("deploy"|"install", default deploy) + gating,
not a file split. Connection panel (discover/trust/fingerprint/key-setup) is shared.
[fleet.rs NEW, 5 tests] FLEET_USER_DEFAULT="gitmind". valid_username (injection guard:
lowercase-start [a-z0-9_-] ≤32). fleet_user()/set_fleet_user() persist ~/.config/gitmind/
fleet-user. base64_encode (std alphabet, known-vector tested). provision_user_script(user,
key_line): portable useradd[Fedora]/adduser[Alpine] + ~/.ssh + key SINGLE-QUOTED (from="
..." double-quotes safe) + echo gitmind-fleet-user-ready. provision_remote_command(b64):
sudo -S sh -c "$(printf %s 'b64' | base64 -d)" (quote-safe transport).
[deploy.rs] restricted_key_line made pub. +ssh_sudo_run(admin,host,password,remote_cmd,
stdin_data): ssh via password-askpass (no TTY) + write stdin to remote cmd (sudo -S password).
14 tests unchanged.
[main.rs] +mod fleet. fleet_user()→{user:fleet_user, admin:whoami}. set_fleet_user(name).
start_provision_fleet_user(admin,host,password) ASYNC job kind "provision" → free fn
provision_fleet_user: ensure_local_pubkey + restricted_key_line(authorized_keys_options) +
fleet::provision_user_script + base64 + provision_remote_command → deploy::ssh_sudo_run
(stdin=password) → check "gitmind-fleet-user-ready". rustcheck CLEAN.
[Fleet.qml] mode toggle (⬆Deploy/⬇Install pills); Component.onCompleted reads fleet_user()
(user=fleet_user, adminUser=whoami) NOT current_user. mode-aware section-2 header; "as user"
field + "save"(saveFleetUser→set_fleet_user) + fleet-user hint. Setup col +divider +provision
block ("sudo as" adminUser + "create fleet user"→provisionFleetUser). applyJobResult +provision
branch (clears sshpass/showProvision + refreshTrust). selectDevice resets showProvision.
GATING: push section visible curPath>0 && mode==="deploy"; "build + install…" button + installer
configurator visible only mode==="install". Fleet.qml {196}= balanced, all backend.* resolve.
[main.qml] tab "deploy"→"fleet"; Fleet{id:fleet}; comment updated. Balanced.
[FLEET-USER.md NEW] why dedicated user (isolation/least-privilege/no-surprise-default), how
managed + provisioned, bootstrap order (provision-first OR install-key-for-existing), restricted-
key tie-in, note Network ssh:// provider still uses login name (follow-up).
VERIFIED: fleet 5, deploy 14, buildflags 6, appcatalog 5, netshare 4, bttransfer 3 = 37 gui tests;
rustcheck CLEAN; Fleet/main/Network.qml balanced; no stale Deploy.qml refs. ON-DEVICE: useradd/
adduser/sudo/ssh/scp/cross-build — provisioning may need per-distro tweaks; bootstrap needs the
fleet user to exist before connecting AS it (provision first).

── app store: Install mode becomes a real storefront ──
USER: still confused by the Fleet UX; wants to try an "app store feel". ROOT CAUSE:
Deploy and Install both rendered as "browse folders → do a thing", so they looked
identical and felt interchangeable. FIX: make Install a card-based app store (looks/
behaves nothing like the deploy folder-push), so the two directions are visually +
interactionally distinct → confusion resolved at the root, not skinned.
[appcatalog.rs +2 tests=7] CatalogEntry{name,size,description}. remote_catalog_cmd(root):
ONE ssh call — for each app subdir of root (Cargo.toml||.git) prints name\tsize(du -sh)\t
blurb(Cargo.toml ^description, README first prose line fallback); root ''|~ → $HOME, ~/* 
expanded; tab-sep. parse_catalog(body) splitn(3,'\t'), drops blank-name lines.
[main.rs] store_catalog(user,host,root): remote_capture(remote_catalog_cmd(shell_quote root)),
first line=resolved pwd→base, parse rest → [{name,size,description,path=base/name}]. 
local_target(): musl-loader fs check (/lib/ld-musl-aarch64.so.1 | libc.musl) → aarch64-musl
else aarch64-gnu — so a store install defaults to a binary that runs on THIS device.
rustcheck CLEAN.
[Fleet.qml] store state (catalog, storeRoot="~", storeBase, storeLoaded, localTarget,
installingApp). Component.onCompleted +local_target(). onModeChanged → auto loadCatalog when
entering install on a connected device. loadCatalog()→store_catalog. installDefault(app)→
install_app(path,name,localTarget,"",no_default=false,stage_tmp=true) — ONE-TAP: native target +
repo default features + /tmp. openInstaller(path) now CARD-DRIVEN (param, defaults instTarget to
localTarget if offered) = the "options" path (full target/feature/cmake control, reused). install
branch clears installingApp. selectDevice resets store state.
GATING: folder browser (path label + entries) + "browse" button now visible only mode==="deploy";
new STORE section visible only mode==="install": "apps in <root>" field + refresh + per-app CARDS
(letter avatar, name, size, 2-line blurb, [Install] one-tap + [options]→configurator). Empty/
unloaded hints. Fleet.qml {225}= balanced, all backend.* resolve.
VERIFIED: appcatalog 7, deploy 14, buildflags 6, netshare 4, bttransfer 3, fleet 5 = 39 gui tests;
rustcheck CLEAN; Fleet/main/Network balanced. ON-DEVICE: the catalog shell (du/grep/sed), cross-
build, scp — store renders from one round-trip; libc detection assumes aarch64 (both devices are).
DESIGN NOTE: store = Install only (browse a catalog, get apps); Deploy stays the push flow. The
look/interaction gap between "cards you install" and "folders you push to" is the clarity win.

── CORRECTION: app pages over local repos (build status → send binary) ──
USER (4th redirect, frustrated): the GUI was meant to be a BRIDGE to local repos with
Cargo.toml/CMakeLists, offering each as an APP PAGE on top of gitmind's repo management;
NAMES from the cargo CRATE (not folder); flow = build → build STATUS → on completion,
only the BINARY is sent (xfer). MY ERROR across prior turns: sourced "apps" by sshing +
browsing a remote filesystem, added deploy/install toggles + a remote store — all wrong
SOURCE + wrong framing. FIX: local-first app pages.
[buildflags.rs +1 test=7] read_section_string + crate_name([package] name) + package_description.
[deploy.rs] scp_push_argv/scp_push (local→user@host:remote, reuses SSH_OPTS + mkdir -p). 14 tests
(push assertion folded into outbound-ssh test).
[main.rs] local_apps(): scan repo root + apps/crates/shared/tools children for Cargo.toml(crate_name
+desc) / CMakeLists → [{name=CRATE,path,description,is_cargo,is_cmake}] sorted; LOCAL, no ssh.
local_build_options(path): local twin of app_build_options (reads local Cargo.toml/CMakeLists →
features+per-arch defaults+cmake opts). build_app(app,target,features,no_default) ASYNC job "build":
runs cargo_build_argv LOCALLY (current_dir=repo root) → on success {binary=root/target/<t>/release/
<crate>, size}; on fail {error=stderr} (= the build STATUS). send_binary(binary,user,host,dest)
ASYNC "send": scp_push binary → ~/.local/bin/<name> (default) + chmod +x; ONLY the binary moves.
local_target() unchanged (musl vs glibc).
[Fleet.qml REWRITTEN 876→462] App list (local_apps cards: crate name + desc) → tap → APP PAGE:
crate header, target pills (default native), collapsible feature options, [Build] → live build
STATUS panel (⏳building / ✓ built+size+path / ✗ error output), then [Send the binary] panel
(dimmed until built) = device picker (scan LAN/select/trust + fleet-user save + install-key/
provision) + Send → status. Reuses connection fns. poller dispatches discover/trust/setupkey/
provision/build/send. {131}= balanced, all backend.* resolve.
DROPPED from UI: deploy/install toggle, remote folder browser, remote app store, push-monorepo /
build-launcher. Backend methods now unused by QML (store_catalog, install_app, app_details,
app_build_options, start_browse, start_push, start_deploy, list_remote_dirs) LEFT IN (qt_method
registered, rustcheck clean) — prune next pass.
VERIFIED: deploy 14, buildflags 7, appcatalog 7, netshare 4, bttransfer 3, fleet 5 = 40 tests;
rustcheck CLEAN; Fleet/main/Network balanced. ON-DEVICE: cross build (local, needs `cross`), scp.
MODEL NOW: apps = local buildable crates; build locally w/ status; send only the binary. This is
the desktop-push direction; phone-initiated fetch = future layer (phone asks desktop to build+send).

── app tiles: screenshot capture + hash-monogram fallback ──
USER: "pick sane defaults and implement". DEFAULTS CHOSEN: (1) screenshots auto-grabbed
after a successful build (best-effort, bounded, blank-frame guard) + manual re-grab;
(2) monogram seeded from each app's OWN per-app commit oid (per-app colour); tile priority
= screenshot if grabbed else hash monogram (flat letter avatar removed).
[preview.rs NEW, 3 tests] cache_dir ~/.cache/gitmind/shots. shot_path(app,oid) keyed app-<oid10>.png.
capture_script(binary,out): Xvfb offscreen 1000x1000 → run binary QT_QPA_PLATFORM=xcb </dev/null →
0.5+1s settle → import -window root → blank guard (convert %[fx:standard_deviation] < 0.02 via awk →
rm + noshot) → echo shot:<path>|noshot; always kills app+Xvfb; binary/out single-quoted. parse_capture
reads trailing shot:. ALL on-device (Xvfb+imagemagick); pure parts tested.
[main.rs] +mod preview. local_apps now adds per app: oid (app_oid = git -C root log -1 --format=%H --
<dir>) + shot (shot_path if cached file exists). capture_preview(app,oid,binary) ASYNC job "shot":
mkdir cache, run capture_script via sh -c LOCALLY, parse_capture → {ok,shot:path|""}. app_oid free fn.
rustcheck CLEAN.
[MonoTile.qml NEW] Canvas: monogram (1 letter single-word / 2 initials hyphenated) from appName;
hue+warp from oid (FNV seed → mulberry32 prng); 2 alpha echoes + crisp front; hsl→rgb helper (Canvas-
safe), "bold" weight. qrc registered.
[Fleet.qml] state +selOid/selShot/shotVer. openApp sets them. list card + app-page header: Image
(file://shot) if shot else MonoTile(appName,oid). build success → auto capturePreview(); "shot" result
→ selShot=res.shot, shotVer++ (cache-bust). re-grab button (visible when built). BusyIndicator while
grabbing. Flat letter avatar removed. {140}= balanced, MonoTile balanced, all backend.* resolve.
VERIFIED: deploy 14, buildflags 7, appcatalog 7, netshare 4, bttransfer 3, fleet 5, preview 3 = 43
tests; rustcheck CLEAN; Fleet/MonoTile/main/Network balanced. ON-DEVICE: Xvfb launch/grab (needs
xvfb+imagemagick; per-distro tuning, window-vs-root crop, 1s timing); monogram is pure QML (no deps).

── fix: build failed with ENOENT (os error 2) ──
USER on-device: "no such file or directory os error 2 when trying to build". CAUSE: build_app
ran Command::new("cross") — cross (a) needs Docker/Podman (unviable on the phone) and (b) lives in
~/.cargo/bin which a Plasma-LAUNCHER-spawned GUI's minimal PATH omits → ENOENT on argv[0].
FIX: cargo_build_argv "cross"→"cargo" (plain cargo --target, no container; native target just works,
other libc needs `rustup target add`). build_app: resolve cargo via cargo_bin() ($CARGO →
~/.cargo/bin/cargo → "cargo") + run with env PATH=tool_path_env() (~/.cargo/bin:/usr/local/bin:/usr/
bin:/bin:$PATH) so cargo + rustc/linker subprocs resolve from a launcher. NotFound → clear message
("install Rust / launch where cargo is on PATH"). capture_preview sh -c also gets tool_path_env (Xvfb/
import/convert). buildflags test now asserts argv[0]=="cargo". DISTRIBUTION.md cross→cargo.
VERIFIED: buildflags 7; rustcheck CLEAN. (Other module tests unchanged.) Native-arch build is now the
zero-setup path; cross-libc target still needs its std (+ musl linker) installed on-device.

── remote build: offload the build to the desktop, install here (app-store feel) ──
USER: phone requests → desktop cross-builds → rsync binary to /tmp → cp to ~/.local/bin (gitmind
→ active user) → .desktop launcher; "feels like an app store." DIRECTION CHOSEN: phone-PULL (phone GUI
as active user ssh's to desktop as gitmind fleet user, builds remotely, rsync-pulls to /tmp, installs
to active user's ~/.local/bin, writes .desktop). Cleaner than desktop-push, reuses the existing
connection, matches "requested by my phone." gitmind = remote build identity on the desktop; active
user = phone GUI user; /tmp = the handoff.
[installer.rs NEW, 2 tests] bin_dir/desktop_dir(home)→~/.local/{bin,share/applications}. desktop_entry
(name,exec,icon)→.desktop text (Icon line only if icon non-empty).
[deploy.rs +rsync, 14 tests] rsync_pull_argv(user,host,remote,local)→[-az,-e,ssh+SSH_OPTS,user@host:
remote,local]. rsync_pull: try rsync; ENOENT → scp_pull fallback (Alpine phone may lack rsync).
[fleet.rs build_repo, 5 tests] config_path generalized → config_file(name). build_repo()/set_build_repo
persisted ~/.config/gitmind/build-repo.
[main.rs] +mod installer. build_repo/set_build_repo methods. remote_install(app,target,features,host,
user,repo,icon) ASYNC "rinstall": (1) remote_capture ssh "cd <repo> && export PATH=$HOME/.cargo/bin:
$PATH && <cargo_build_argv joined>" — cargo error returns as ssh stderr (NO 2>&1). (2) rsync_pull repo/
built_binary_path → /tmp/<app>. (3) fs::copy /tmp→bin_dir/<app> + set_executable + rm /tmp. (4) write
desktop_entry → desktop_dir/<app>.desktop (icon=selShot if any). returns {app,binary,launcher} or
{stage,error}. no_default = features non-empty. rustcheck CLEAN.
[Fleet.qml] state +buildHere(true)/repoField/rinstallState/rinstallStage. openApp loads build_repo.
doRemoteInstall (saves repo, calls remote_install, job "rinstall") + saveBuildRepo. applyResult
"rinstall" branch. UI: "build on: [this device|desktop]" toggle. Local Build button gated buildHere.
Panel 2 repurposed both modes: buildHere→"Send the binary" (after build); !buildHere→"Build on the
desktop" (repo field + host picker reused + "Build & install" → enabled when host trusted + repo set).
device picker (discover/trust/fleet-user) now shared by both modes (visible when !buildHere || built).
{160}= balanced; all backend.* resolve.
VERIFIED: deploy 14, buildflags 7, appcatalog 7, netshare 4, bttransfer 3, fleet 5, preview 3,
installer 2 = 45; rustcheck CLEAN; Fleet/MonoTile/main/Network balanced. ON-DEVICE: remote cargo build,
rsync (scp fallback), cp, .desktop — needs cargo+target on desktop, ssh as gitmind, source repo on
BOTH (phone browses, desktop builds). Push-vs-pull: chose pull; desktop-push variant addable.

── fix: monorepo subpaths — build in the app's own dir, find binary via metadata ──
USER: apps in big monorepos build from the TOP dir (e.g. tmp11) and look for Cargo.toml there;
apps live at apps/{gitmind,…} with their own Cargo.toml → "could not find Cargo.toml in tmp11".
CAUSE: build_app ran cargo with current_dir=repo root + `-p <name>` (assumes a root-workspace member)
and computed the binary as root/target/<t>/release/<app>. Wrong for standalone subdir crates.
FIX: build IN the app's directory. build_app gains a `path` param (the subdir) → current_dir(path).
Binary located via cargo metadata `target_directory` (correct for standalone crates AND workspace
members — workspace member → workspace target, standalone → its own target), fallback <app_dir>/target.
[buildflags.rs +2] json_string_field(blob,key) (std-only substring extractor, handles \" \\, null) +
parse_target_directory(meta). 8 tests.
[main.rs] resolve_binary(cargo,app_dir,triple,app): cargo metadata --no-deps in app_dir → target_dir →
target_dir/<triple>/release/<app>. local_apps +`rel` (path relative to repo root). build_app(app,PATH,
target,feats,nodefault) cd's into PATH. remote_install(app,REL,target,…): app_dir = repo/rel; remote
`cd <app_dir> && cargo build`; remote binary located via remote `cargo metadata` in app_dir → target_dir
(workspace-aware on the build host too). (legacy unused install_app still uses built_binary_path — dead
code, left for the prune backlog.)
[Fleet.qml] +selRel (from app.rel in openApp). doBuild passes selPath; doRemoteInstall passes selRel.
VERIFIED: buildflags 8; deploy 14, appcatalog 7, netshare 4, bttransfer 3, fleet 5, preview 3,
installer 2 = 46; rustcheck CLEAN; Fleet/MonoTile/main balanced; all backend.* resolve. ON-DEVICE:
cargo build + metadata in the subdir (local), and over ssh in repo/rel (remote).

── fix: target detection, lib crates in store, repo path dropdown ──
USER on-device: (1) desktop is glibc but GUI recommended musl (musl loader file present alongside
glibc → false positive); (2) share-kit (a LIBRARY, libshare_kit.rlib, no bin) was listed + "installed"
→ rsync exit 23 (no binary); (3) typing the remote repo path from memory is bad UX → want dropdown
+custom; (4) rustup installed but not on PATH (user env; cargo already surfaces "target may not be
installed").
FIXES:
[A target] local_target now runs `rustc -vV` (PATH-augmented) → host_triple_from_rustc (the host: line)
= authoritative native triple. Fallback (rustc unreachable): /etc/alpine-release → musl else gnu. NO
more loader-file guessing. [buildflags host_triple_from_rustc, tested]
[B lib filter] local_apps skips cargo crates with no binary: lists only if src/main.rs || src/bin/ ||
declares_binary(Cargo.toml has [[bin]]). share-kit & other kits no longer appear as apps.
[buildflags declares_binary, tested]
[C repo dropdown] fleet build-repo is now a most-recent-first HISTORY: build_repos()/build_repo(first)/
set_build_repo(prepend+dedupe+cap8). build_repo() method returns {repo, candidates=history+local repo
root}. Fleet.qml: repo field is an editable ComboBox (model=repoCandidates, currentIndex 0, onEditText
→repoField); openApp loads candidates + preserves a typed-but-unsaved path. [fleet 5 tests incl history]
[D error] remote_install fetch-stage failure now appends a hint: "no binary at <path> — is <app> a
library crate, or did the build target a different libc than <target>?".
VERIFIED: buildflags 9, fleet 5, deploy 14, appcatalog 7, netshare 4, bttransfer 3, preview 3,
installer 2 = 47; rustcheck CLEAN; Fleet/MonoTile/main/Network balanced; all backend.* resolve.
NOTE: user's rustup-not-on-PATH is an env issue (rustup target add needs rustup reachable); cargo's own
"target may not be installed" message already guides. Native target now defaults correctly so the
common (this-device) build needs no rustup target add at all.

── native musl builds, kit separation, search/filter, cross→emulator dir ──
USER on-device: phone rustc host = aarch64-ALPINE-linux-musl (vendor triple!), desktop = aarch64-
unknown-linux-gnu. KEY INSIGHT: passing --target aarch64-unknown-linux-musl on Alpine FAILS (that
rustup target isn't installed; musl is the NATIVE host) → that's why musl wouldn't compile. Also: want
kits buildable but separated; suggest switching target when device libc mismatches + install cross
binaries to a separate emulator dir; want grouping metadata + GUI filter + fuzzy search.
FIXES:
[buildflags, 11 tests] cargo_build_argv("" triple) → NATIVE build, NO --target (works on Alpine musl +
Fedora gnu without rustup targets). target_libc(triple)→musl|gnu. normalize_native(host)→aarch64-
unknown-linux-{libc} (alpine→unknown). is_native_build(req,native)=same libc.
[main.rs] native_triple_local() = rustc -vV host → normalize (fallback /etc/alpine-release). local_target
returns it (pill matches now). build_app: cargo_target = "" if native else triple; resolve_binary(cargo,
dir, cargo_target, app) → native lands in target/release (NO triple subdir), cross in target/<triple>/
release. remote_install: query BUILD HOST rustc host → decide native/cross THERE (desktop gnu building
musl = cross → --target); install dir on THIS device via is_native_build(target, native_triple_local):
native→~/.local/bin + .desktop; non-native→installer::install_dir = ~/.local/lib/gitmind/<triple>
(emulator, NO launcher). local_apps: include libs (is_bin flag, was skipping) + group (rel first seg).
[installer.rs, 3 tests] install_dir(home,triple,native).
[Fleet.qml] search TextField + kind pills (All/Apps/Kits) → shownApps=filterApps(apps,search,kind)
(substring OR subsequence fuzzy over name/desc/group). "kit" badge + group label on cards. cross-target
warning banner (target libc ≠ device libc) with "use <native>" switch button. Panel-2 gated to selIsBin;
libs show a "Library (kit)" note (build only, no install). selIsBin from app.is_bin.
VERIFIED: buildflags 11, installer 3, fleet 5, deploy 14, appcatalog 7, netshare 4, bttransfer 3,
preview 3 = 50; rustcheck CLEAN; QML balanced; backend.* resolve.
DEFERRED (told user): richer faceted filters (size after build, author/email, GUI-vs-CLI, feature) +
gitmind ProjectCard grouping schema — this turn does kind/group/search foundation.

── shared target cache, remote repo discovery (peek install bug), weekly key expiry ──
USER: (BUG) installing peek on phone → "zsh cd no such dir /home/USER/Code/.../tmp13/tools/peek" on
the desktop — the dropdown offered the PHONE's local repo root (/home/user/...) but the desktop is
/home/kesslerd/...; build cd failed, nothing rsynced. (FEAT) dedup target/ to a shared cache for repos
with .gitmind. (FEAT) weekly key expiry + settings/cleanup page. (FEAT) a desktop→phone push page.
.gitmind ALREADY EXISTS as a directory marker in the repo; target/ already gitignored.
FIXES THIS TURN:
[BUG] build_repo() NO LONGER adds the local repo root (it's the phone's path, wrong on the host). New
remote_repo_candidates(host,user): ssh `find $HOME -maxdepth 6 -name .gitmind | head -50` → strip
/.gitmind → real HOST repo roots. Fleet.qml selectDevice → discoverRemoteRepos() merges them into the
dropdown (when !buildHere). Hint now says connect as the account that OWNS the source (needs repo+cargo)
— gitmind fleet user has an empty home, so build host should be your desktop user (kesslerd).
[shared cache] gitmind_root(dir) walks up to a `.gitmind` marker. shared_target_dir = ~/.cache/gitmind/
cargo-target. build_app sets CARGO_TARGET_DIR there for gitmind repos (deps build once across tmpN
clones). resolve_binary takes target_dir_env (sets it on the metadata query too → resolved path matches).
remote_install exports CARGO_TARGET_DIR=$HOME/.cache/gitmind/cargo-target on the build host.
[key expiry, deploy 14] KEY_EXPIRY_DAYS=7. key_expiry_time(now,days)→YYYYMMDDHHMMSS (civil-from-days,
tested incl 1700000000→20231114221320). authorized_keys_options(host, Option<expiry>) prepends
expiry-time="…". setup_key installs a 7-day key (renew by re-running install). NOTE: re-install appends
(cat >>) → expired dup lines accumulate; cleanup is the deferred settings page.
VERIFIED: deploy 14, buildflags 11, fleet 5, installer 3, appcatalog 7, netshare 4, bttransfer 3,
preview 3 = 50; rustcheck CLEAN; QML balanced; backend.* resolve.
DEFERRED (told user, planned): (E) dedicated desktop→phone PUSH page [reuse send flow + app picker].
(F) key SETTINGS page: list managed keys (id/host/expiry), de-dupe/cleanup authorized_keys, last-
accessed + network-seen (needs sshd log parsing or a wtmp/last read on-device). (G) reclaim: rm in-repo
target/ dirs now that builds go to the shared cache.

── desktop → phone Push page ──
USER (chose next): a dedicated page for putting binaries on phones from the desktop, like the fetch UX.
[backend] build_locally(app,dir,target,feats,nodefault)→(bin,size) EXTRACTED from build_app (build_app
now calls it; push reuses it — no dup). push_install(app,path,target,features,host,user) ASYNC "push":
build_locally (cross to device libc if needed) → deploy::scp_push bin → host:/tmp/<app> → remote_capture
installer::push_install_script(app,false) (mkdir ~/.local/bin + ~/.local/share/applications; mv /tmp →
bin; chmod; write <app>.desktop with $HOME-expanded Exec via unquoted heredoc; echo gitmind-pushed).
installer::push_install_script(app,has_icon) pure+tested (icon path moves /tmp/<app>.icon → cache when
has_icon). remote_target(host,user)→device native triple (rustc -vV → normalize) so push defaults to the
device's libc. [installer 4, all 51 tests]
[shared, no dup] qml/applib.js (.pragma library): fuzzy/filterApps/targetLibc. Fleet now delegates its
fuzzy/filterApps/targetLibc to App.* (logic lives once). MonoTile reused.
[Push.qml NEW tab] own state+poller (discover/trust/setupkey/push). Compact device picker (discover/
select/trust/install-key — NO provision; simpler than Fleet's). selectDevice auto-detects target via
remote_target. app search+list (App.filterApps, kind app) → tap selects → "for <libc>" pills + "Build &
push" (enabled when app+trusted host). main.qml: new "push" tab between fleet + network. qrc: Push.qml,
applib.js.
VERIFIED: installer 4, deploy 14, buildflags 11, fleet 5, appcatalog 7, netshare 4, bttransfer 3,
preview 3 = 51; rustcheck CLEAN; Fleet/Push/MonoTile/main/Network balanced; all backend.* resolve.
ON-DEVICE: build (cross musl from gnu desktop needs the musl target+linker), scp_push, remote install
script. Connect as the device's run-user. NOTE/TRADEOFF: Push has its own compact device picker (markup
overlaps Fleet's) — extracting a shared stateful DevicePicker.qml deferred (riskier untested); filter
logic IS shared via applib.js. Still DEFERRED: key settings/cleanup page; reclaim in-repo target/ dirs.
