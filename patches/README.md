# patches/ — pending patches from sister sessions

The monorepo's apps are point-in-time snapshots. Active work also happens in standalone
repos and parallel Claude sessions (the snapclone egui-UI port; the logscope
distributed-fleet session). Rather than constantly re-sync, that work lands here as
**patch + handoff pairs**, serving two purposes:

1. **Pending application** — work to eventually fold into the matching app and/or the
   standalone repo it targets.
2. **Source material** — implementations to mine when building out `shared/` kits. When a
   pattern appears in a patch *and* recurs across the monorepo, it's a shared-kit
   candidate. (This is how `net-kit` got started — see the extraction log below.)

Each entry is `<name>.patch` (the consolidated `git diff`) plus `<name>.HANDOFF.md` (its
spec / API reference / integration plan). **These are reference + queued work; they are
NOT auto-applied** — most target a standalone repo whose tree differs from the monorepo
snapshot, so apply them against that repo (byte-exact, from the source) rather than here.

## Index

### `logscope-distributed-fleet`  — logscope session, "speaker reactions in the AI tree"
`logscope-distributed-fleet.patch` + `logscope-distributed-fleet.HANDOFF.md`.
Targets standalone **logscope-suite**, `crates/logscope-egui` (the monorepo's
`apps/logscope` is a snapshot). A complete, unit-tested protocol + scheduler layer for
multi-machine fan-out — 8 always-on modules (117-test lean suite green per the handoff):
`enlist` (join protocol + `Transport` + `UdpTransport` + wire codec + token auth),
`dispatch` (capacity-aware scheduler), `bridge` (skeleton↔Task), `ollama` (dep-free HTTP
client executor), `localexec` (`BackendExecutor`), `speculate` (anticipation lanes +
`LeadCache`), `voicefan` (speaker-attributed utterances → per-speaker tree fanout — the
speaker-reactions core), `devloop` (decompose→fan→verify→assemble loop).
**Still pending per the handoff** (not in the patch): the headless `logscope-node` binary,
the whisper + speaker-stack IO adapter, and the reaction **classifier** (transcript →
Steer/Interrupt/Affirm/AddContext flipping `Backend::generate`'s cancel) — the "steer an
in-flight generation" half of the feature.

## Extraction log  (patch → shared kit)

| shared/ target | lifted from |
|----------------|-------------|
| `net-kit::auth` | enlist `ct_eq`/`frame`/`unframe` + (monorepo) netserve `ct_eq`/`generate_key`/`Keyring` |
| `net-kit::wire` | enlist `esc`/`unesc` percent line codec |
| `net-kit::json` | ollama `json_escape`/`find_string_field`/`read_json_string` |
| `net-kit::http` | ollama `build_request`/`parse_response`/`http_roundtrip` |

Candidates noted, not yet extracted: a `fleet-kit` (dispatch scheduler + enlist join
protocol + bridge transforms) if the fleet pattern recurs beyond logscope; the
`speculate`/`voicefan` anticipation+fanout layer if it generalizes past the AI-tree UI.
