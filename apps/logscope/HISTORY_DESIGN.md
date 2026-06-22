# Project history & branching — design

*Conclusions from the discussion, and what Phase 1 ships. The short version: not git as the backend; a
local content-addressed store with a **tree** of nodes, where every file version is linked to the **turn
and reasoning** that produced it. Git is an optional export, not the foundation.*

## Why not git as the backend

Three concrete reasons, all specific to this app:

1. **Large files.** logscope ingests logs — multi-GB inputs are normal. Git copies and delta-compresses
   everything into history (permanently; rewriting history to remove a big blob is painful), and loads
   objects into memory — on a 32 GB UMA box already holding the model + KV, a repack can OOM the engine.
2. **A safety mechanism must not fail.** Branch points are the safety net. Git can fail (not installed, a
   repack OOM, a big file slipping in) — a safety net that can fail isn't one.
3. **Diff ≠ provenance.** Git tracks *how bytes changed*. We want *why* — this file became that file
   because of the reasoning in turn N. Files here can change in ways line-diff can't track (a rewrite, a
   regenerate), and the explanation already exists: the turn that produced it. Git can't link those.

## The key insight that makes it cheap

Generated files are a **deterministic function of the transcript** (`parse_artifacts(message)`). So the
conversation history *is* the repo for the generated project — version the (small, text) transcript and
re-derive files. The only thing that can be 1 GB is **external input** (logs/attachments), which is never
committed: it's tracked **by reference** (hash + size + mtime), bytes copied only under a size threshold.

## Data model (content-addressed tree)

- **Blob store (CAS).** Everything content-addressed by a 128-bit id (two seeded xxhash passes — zero new
  deps; blake3 is the upgrade for cross-machine/export trust). Identical content stored once → a tree with
  deep history and many branches stays small.
- **Node = a commit = a tree point.** `{ id, parent, children, label, cum_len, added: [MsgRef], files:
  [FileEntry], inputs: [InputRef], note }`. `added` is the *message delta vs parent*, so a child shares all
  of the parent's message blobs (prefix sharing) and adds only its new turns. The path root→node is the
  full transcript — and is exactly a KV prefix (see below).
- **FileEntry carries provenance, not just content.** `{ path, hash, kind, fragments, produced_by, turn }`
  where `produced_by` is the blob hash of the message that generated the file. "Why is this file like
  this?" → `get_text(produced_by)` = that turn's full reasoning. `file_history(node, path)` walks the path
  and returns the ordered versions where content actually changed, each linked to its producing turn — a
  causal chain, not a diff.
- **InputRef = reference, never the bytes (if large).** `{ path, size, hash, mtime, present }`. Hashed in
  chunks (never loaded whole), so a 1 GB log is identified, not copied. This is the 1 GB cliff handled.

On disk under `~/.local/share/logscope/vault/`: `blobs/<2>/<hash>`, `nodes/<id>.json`, `index.json`
(head, next_id, roots, branch labels). Individual blob files → seekable, mmap-able, concurrent reads, no
single giant file to parse. Writes are atomic (tmp + rename) for crash safety.

## How the tree connects to the engine (later phases)

A node is a conversation prefix; a prefix is a KV-cache state; we already built the structure that forks KV
states cheaply (the coordinator's prefix trie: `cache_prefix` / `extend_prefix` / `seed_from_prefix`). So:

- **Parallel branches** = the active leaves of the tree are the coordinator's active sessions (forked from a
  node's tokens, decoded batched under the KV budget). Mostly wiring, not new kernels.
- **Branch a tool a month later** = the node persists (tokens + file manifest); fork a child and **re-prefill
  the stored prefix** to rebuild KV. Optional accelerator (Phase 3): persist an int8-KV snapshot per node
  keyed by model fingerprint for warm-start, with re-prefill as the always-there fallback.
- **Tree-defined context** = spawning from a node uses the root→node path as the prompt prefix.

The store holds `(node, token offset, tokens, files, reasoning)`; the engine turns that into KV. The
database does **not** hold a live KV cache — that's a runtime GPU structure — but the tree *drives* it.

## Phasing

1. **Store + tree + provenance** (this build): the vault, auto-commit on each completed turn, return/branch,
   per-file history with reasoning. No engine changes. Fully unit-tested.
2. **Parallel branches**: wire live leaves to the coordinator (WorkerPool).
3. **KV warm-start**: persisted int8-KV per node, bit-exact gated; re-prefill fallback.
4. **Export**: git + LFS of a chosen branch, on demand (LFS for the referenced large inputs).

(Code-AST navigation — tree-sitter, jump-to-symbol — is an orthogonal, language-specific, heavy-dep layer,
deferred. "Structured access" in Phase 1 = turns / fences / files, which we already segment.)

## Phase 1 limitations (honest)

- Editing a message *inside* an already-committed prefix isn't re-tracked; new turns are. (Re-parenting on
  edit is a later refinement.)
- The 128-bit xxhash id is for local dedup/integrity, not adversarial collision resistance — fine locally;
  blake3 is the export-grade upgrade.
- Vault logic is unit-tested in the sandbox; the UI (history panel, return/branch, file-reasoning view) is
  compile-validated here and exercised on the M2.
