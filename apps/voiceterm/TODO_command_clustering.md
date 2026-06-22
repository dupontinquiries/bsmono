# TODO: command/session clustering → branch labels (R5)

## Priority: HIGH-INTEREST but PROTOTYPE-FIRST (most novel, most uncertain)

## The idea (user)
Treat captured voice/command/session data in the git repo as a dataset;
run k-means (flat labels) over it; abstract the clusters up to BRANCH-level
groupings. Discover the natural "task clusters" in how the user works and
map them onto git branches — auto-grouping sessions/commands by what they
were really about. Cluster labels could NAME sessions/branches (pairs with
R2 auto-naming).

## HONEST caution (validate before claiming)
k-means needs a real feature representation of "what a command/session was
about." On a small dataset, clusters may NOT align with anything human-
meaningful — it can produce labels that LOOK like insight but aren't. So:
PROTOTYPE + MEASURE before building into the app, exactly like the 39-dim
speaker embedding (proved 20x separation before touching live code).

## Prototype plan (in-sandbox, pure Rust)
1. Define a feature vector per command/session: e.g. bag-of-tokens over the
   command + cwd + touched files + dominant verb, hashed/embedded to a
   fixed dim (or TF-IDF over the command corpus).
2. k-means (plain Rust, no deps, or candle) over synthetic-but-realistic
   command sets that have KNOWN latent groups (e.g. "git ops", "build/test",
   "file nav", "ssh/remote").
3. MEASURE: do the recovered clusters match the known groups? (purity /
   adjusted-rand vs the planted labels). Only proceed if separation is real.
4. Label generation: most-distinctive tokens per cluster → a flat label;
   abstract to branch by mapping the active branch's sessions to their
   dominant cluster.

## If it validates
- Feed cluster labels into R2 (session auto-naming) and a "branches as task
  clusters" view.
- Pure k-means + features = compile + fuzz verifiable here.

## Status: not started. BUILD AS A PROTOTYPE + MEASURE first; do not wire
## into the app until clusters demonstrably separate.
EOF

## PROTOTYPE RESULT v0.1.661 (measured — read before wiring)
Built src/cmd_cluster.rs: hashed bag-of-tokens features (64-dim, FNV, L2-
normalized) + deterministic k-means++ + purity metric + cluster-label
extraction. 4 tests (the validation is permanent).

### Measured on planted 5-task data (git/build/python/filenav/ssh)
- purity = 0.80 (best of 8 seeds) — MARGINAL, not the decisive separation
  the 39-dim speaker embedding showed (20x). It clears a 0.80 bar but no
  more.
- cluster labels came out SENSIBLE: ["python","git","cargo","m1n1","ls"] —
  these do map to the planted task types.

### What clusters WELL vs POORLY (the honest finding)
- WELL: tool/verb-based tasks with a strong shared token — git*, cargo/make,
  python* cluster cleanly.
- POORLY:
  * behavioral categories (file-nav: ls/cd/cat/find share almost no tokens,
    so they don't form a lexical cluster — file-nav is a CONCEPT, not a
    vocabulary),
  * arg-dominated commands: ssh split into two clusters by HOSTNAME
    (m1n1 vs build-box) instead of grouping as "ssh"; cwd token can dominate
    short commands (ipython landed with python by shared cwd).

### Verdict: concept works, but lexical features alone are too weak to wire
as-is. Before any app wiring, improve the representation:
  1. down-weight/stop-list high-cardinality args (hostnames, hashes, line
     numbers) and the cwd token, OR weight the VERB (argv[0]) higher;
  2. add behavioral features (reads-files? network? writes? builds?) so
     file-nav becomes a cluster by BEHAVIOR not vocabulary;
  3. then re-measure purity — target >0.90 before wiring labels into
     session_auto naming.
Until then: NOT wired. The pure core + the measurement harness stay so the
representation work can be measured against the same planted bar.

## v0.1.665 — representation improved + RE-MEASURED (now wireable)
Did the rep work the prototype prescribed, measuring each change on the same
planted data (best-of-N seeds):
  - v1 lexical bag-of-tokens + cwd:           0.88  (more seeds than v661's
    8 already lifted the original "0.80" — partly seed luck)
  - verb-weight (3x) + DROP cwd:              0.80  (WORSE — dropping cwd
    lost the repo signal; verb-weight can't fix file-nav, whose verbs all
    differ: ls/cd/cat/find)
  - + down-weight high-cardinality args:      0.80  (no change here)
  - + BEHAVIORAL category prior (verb→cat):   1.00  (decisive)
Adopted the behavioral representation in featurize(); bar raised 0.80→0.95.

### Honest framing (important)
The 1.0 comes from a CURATED verb→category prior (verb_category): git→vcs,
ls/cd/cat/find→filenav, ssh→remote, etc. So this is SEMI-SUPERVISED, not
pure discovery — file-nav is a *behavior*, not a shared vocabulary, so no
unsupervised lexical trick clustered it (measured: verb-weighting didn't).
That's a legitimate design choice (a small known-verb taxonomy + lexical
fallback for unknown verbs), but the win is the taxonomy, not k-means
"discovering" structure. k-means still groups + handles unknown verbs via
lexical features.

### Now wireable (was blocked at 0.80)
Clears the 0.95 bar → cluster labels can feed session_auto naming + a
"branches as task clusters" view. Wiring: featurize the session's commands,
kmeans, cluster_labels → name. Still no app wiring yet (next slice), but the
representation question is RESOLVED + measured.

## WIRED v0.1.720 — backend qt_method live
- cmd_clusters(k) qt_method: featurizes the command-block history (validated
  hashed-token features), runs k-means (k = requested or ~sqrt(n) in [2,6]),
  returns the most-distinctive label per cluster. Verified: a git-ops + python
  mix split into ["git","py"] with the git commands all in one cluster.
- REMAINING (QML): a "task clusters" view / branch-label surfacing; and feeding
  the dominant cluster label into session_autoname. Backend ready.

## WIRED v0.1.721 — feeds the session name (R5 → R2)
- _dominant_cluster_label() picks the largest cluster's label (gated: >1 cmd and
  >1/3 of the stream, so a noisy split doesn't mislabel) and session_autoname
  appends it. The "cluster labels could NAME sessions" pairing, realized.
