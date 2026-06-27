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
