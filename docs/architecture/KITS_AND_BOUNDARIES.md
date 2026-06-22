# Kits, FFI boundaries, and shared libs — planning (first pass)

~600K LOC across the tree has real overlap. The goal is fewer, well-bounded
shared pieces — to cut maintainer fatigue and make the 600K *maintainable*, with
the LOC trending **down** over time. This is the inventory + a keep/oxidize/
kit-ify classification + how it reshapes the build. It is a plan; each step is
its own work, validated on hardware. Tracked as scratchpad **G1**.

## 1. FFI boundaries in the tree

Seams where two sides change independently (the only places that can "drift").

| boundary | where | mechanism | verdict |
|---|---|---|---|
| Rust ↔ C++ DataBus | **snapclone** (21 cdylib/staticlib crates, 30 `#[no_mangle]`) | hand-written C ABI | **OXIDIZE** — the whole C++ burden lives here |
| Rust ↔ Qt | voiceterm, gitmind-gui, snapclone (C++ Qt) | qmetaobject cdylib | **KEEP now → migrate to a shared egui `uikit`** (G2) |
| C++ gui ↔ Rust daemon | photosync | C++ UI over a Rust core | oxidize-candidate (small C++) |
| Rust ↔ Vulkan | unendlich | GLSL → SPIR-V (`.spv`) | KEEP — not FFI; fix the `.spv` build (compile-on-build) |

The headline: **all hand-written C++ FFI is in snapclone.** Oxidizing it (C1)
both removes the C++ and makes snapclone cross-clean (the C++ is the cross-pain
for the aarch64-host → x86/riscv targets).

## 2. Shared-lib / kit candidates

Pure-Rust reusable pieces worth lifting to a top-level `shared/` (or `kits/`).

| candidate | source | reuse | notes |
|---|---|---|---|
| **registry-kit** (linkme autoloader) | unendlich `inventory.rs` + `*_registry*` | component/system/kernel auto-registration with **no central list** | `linkme` 0.3 distributed slices; self-contained (pulls nothing else in); user-flagged for reuse — **highest-value, lowest-risk lift** |
| **peek-view** ✅ | tools/peek | file-viewer lib used by gitmind-gui | **done**: tools/peek promoted to lib+bins (v0.2.0); gitmind-gui consumes it; embedded copy removed. Stayed in tools/ (it's also the wiring-audit suspender), not shared/ |
| **net-sim** | snapclone net-intel/diag + bo-suite `network_sim` | shared CI network-simulation harness | cf. scratchpad E1 |
| **snapcore-protocol** (future) | ops-doc | the DHT wire/protocol crate | the shared lib that lets snapclone + unendlich share the DHT |
| experimentkit / gesturekit | bot-arena | already kit-shaped | lift / standardize |
| **uikit** (egui) | fresh; carries snapclone's swipe-gesture UX | one egui UI layer across all apps, replacing Qt | think fresh on Qt→egui (don't pattern-mine the old GTK→egui logscope port); lighter than Qt → also helps RISC-V viability |
| render-kit (?) | unendlich ⨯ logscope overlap | shared GPU/render pieces | investigate the overlap before extracting |

`gitmind-core` isn't cross-app, but it's the **pattern exemplar**: one core, thin
CLI/GUI frontends that "can't drift". Apply that shape when kit-ifying.

## 3. How kits/FFI reshape the build

This is where the build shape changes (the user's intuition was right):

- **Build variants via cargo features.** Integrated systems run snapclone +
  unendlich **sharing the DHT**; playtesters want unendlich **without snapclone
  bloat**. Mechanism: the DHT becomes a shared lib (snapcore); unendlich gains an
  optional `dht` feature that pulls it in. Build profiles / flatpak select the
  variant. The orchestrator grows a `--variant {integrated,lean}` notion.
- **The DHT-as-shared-lib is the linchpin** — it's simultaneously an oxidation
  target (C1/D1) and the enabler of the integrated-vs-lean split. Decomposing it
  (D1) into a Rust `snapcore` crate makes that split clean.
- **Target matrix.** Host is aarch64 (Asahi, 16K pages); games must also reach
  x86 and riscv (cross-only — no x86 hardware). Pure-Rust kits cross trivially;
  snapclone's C++ does not → another reason to oxidize. Mind the 16K-vs-4K page
  difference between the aarch64 desktop and phone.
- **New top-level layer.** A `shared/` directory of lifted crates consumed by
  path turns the build from "N independent projects" into "N apps + a shared kit
  layer" — fewer copies, one place to fix each concern.

## 4. First-pass classification

- **OXIDIZE:** snapclone C++ DataBus → Rust (leaf-first); photosync C++ gui (maybe).
- **KIT-IFY → `shared/`:** registry-kit (linkme), peek-view, net-sim,
  experimentkit/gesturekit.
- **KEEP:** Qt qmetaobject boundaries (voiceterm, gitmind-gui); unendlich Vulkan/GLSL.
- **DEPENDS:** the DHT → decompose to a `snapcore` Rust kit; enables variants.

## 5. Recommended sequencing

1. ✅ **Lift registry-kit** (linkme) — done: `shared/registry-kit`, built+tested.
2. ✅ **peek-view reconcile** — done: tools/peek promoted to lib+bins; gitmind
   consumes it; embedded copy removed (stayed in tools/, not shared/).
3. **Decompose the DHT → `snapcore`** + define unendlich's `dht` feature — this
   is where build shape changes most (integrated vs lean).
4. **Begin snapclone C++→Rust oxidation**, leaf-first, shrinking the FFI. As you
   port the UI, carry its swipe-gesture UX into the egui `uikit` (cpp&qt →
   rust&egui), rather than porting Qt 1:1.

The egui **uikit** (G2) is a parallel track to this sequence, not a blocker —
it can start whenever, and snapclone is its first real consumer.

Each lands as its own change, built/tested on hardware (Qt, cross, GPU).
