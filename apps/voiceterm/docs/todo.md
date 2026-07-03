# TODO

Curated active TODO list. Grouped by area, sized roughly.

Sizes: **S** = ~1 hour, **M** = ~one session, **L** = multiple sessions.

For historical per-version notes, see `../TODO_fingerprint_db.md`.
For active development, this file is the canonical list.

For manual / human-driven verification, see
[`test-sessions/`](test-sessions/) — currently a
power-user walkthrough covering v0.1.349–v0.1.358.

## Mail bridge — ProtonMail (v0.1.865–869)

Hydroxide-derived bridge incorporated and compiling across all three
feature paths (default/inert, `voice claude`, `mail-crypto`); 78 module
tests pass in-tree. The **pre-auth surface is complete**: login form with
in-pane status feedback (`web2qml://mail/login`), the "add my email"
Proton-vs-not voice command, and the accounts settings pane
(`web2qml://mail/settings`) with add/remove. Backend methods live:
`mail_open_login`, `mail_login`, `mail_remove_account`,
`mail_register_email`, `mail_open_settings`, lazy `mail_hub_get`.

Everything past authentication is gated on crypto seams the bundle left
as TODO. These need an environment where they can be verified against the
real Proton service (test vectors or a live account); they are NOT safe to
write blind — auth crypto with silent, security-relevant failure modes.

- [ ] **L — SRP login seam** (`mail_crypto.rs mod real::ProtonSrp`).
  Proton SRP gotchas: little-endian bignum serialization, bcrypt-variant
  password preprocess (`$2y$`), custom ExpandHash (SHA-512 ×N), and a
  PGP-signed modulus that must be verified against Proton's pinned key
  (depends on the PGP seam below). Options: add `proton-srp` crate (carries
  its own tests) or port go-srp against authoritative vectors. Until this
  lands, `mail_login` returns `err\tCryptoUnavailable` (now shown in the
  form's status line). **Blocker for all authenticated mail.**
- [ ] **L — PGP read seam** (`mail_crypto.rs mod real::SequoiaPgp`):
  unlock address keys + decrypt bodies/attachments. Needs
  `sequoia-openpgp` or `pgp`. Also required by the SRP modulus check.
- [ ] **M — send seam** (`mail_proton.rs` send_message / SMTP submission).
- [ ] **M — post-auth UI wiring** (unblocked only after SRP+PGP): inbox /
  message / compose / attachment panes, voice grammar via
  `MailHub::handle_phrase` + `PaneAction`→pane mapping (see
  `BACKEND-WIRING.md`), and the notifier timer (`poll_new_mail` /
  `prime_notifications`).
  - v0.1.893 — built the message-VIEWING voice grammar ahead of the panes:
    `mail_intent::parse_message_nav` + `NavDir{Next,Previous,Back,Close}` turn
    spoken "next message" / "go back one" / "back to the list" / "close this"
    into a direction the UI maps onto its message-pane stack once those panes
    exist. Pure + unit-tested (incl. the "back one"=Previous vs bare "back"=
    Back-to-list overlap). Not session-gated — navigation is stateless pane
    movement, so it'll wire the moment the message pane lands (no auth needed
    for the nav itself). Still TODO: a `mail_message_nav` qt_method + an
    executeIntent branch that calls it only while a message pane is focused.
  - v0.1.893 — also broadened the LIVE sign-in voice command
    (`parse_register_email` / the `mail_register_email` qt_method, wired in
    executeIntent today): it matched only the exact strings "sign in to email"
    / "log in to email", so "sign into my email", "log in to proton", "sign me
    in to my inbox" fell through. Now a sign-in verb + a mail-specific noun
    matches (currency-/bank-safe: "sign in to my account" with no mail noun is
    deliberately excluded). Opens the Proton login form — the one mail UX that
    works pre-auth.
- [ ] **S — async login** so `mail_login`'s blocking ureq round-trip
  doesn't run on the UI thread (spawn + signal the result wire back).

## Exploratory direction (v0.1.174 conversation, not committed)

The tool builds toward a personal computing substrate where cards
(web excerpts, terminal commands, voice notes, files) accumulate on
a canvas, with git as the underlying history layer and voice
attribution as a passive identity mechanism. Built for single-user
first; the multi-user case (kid + parent on the same device,
eventually) falls out of the architecture without additional design.
The browser, terminal, voice, and archive components we've built
are pieces of this future canvas; the immediate work (voice corpus
5-ship arc) establishes the git substrate that everything else
depends on.

This direction emerged across a long conversation in May 2026 and
is filed here as preservation, not commitment. It may or may not
survive reflection. The active arc (voice corpus, 5 ships) is
unchanged and still mission-critical regardless of whether this
framing holds.

## Analytics-flip (v0.1.177 conversation, exploratory)

The same signals that web analytics systems collect (scroll
velocity, dwell time, click patterns, mouse paths) can be used for
the opposite goal: optimizing for the user instead of extracting
attention from the user. The mechanism is identical; the direction
of optimization is reversed.

  - Engagement analytics: dwell time = ad inventory served.
    Care analytics: dwell time = "this is what you cared about,
    want it captured?"
  - Engagement: scroll velocity = "show more like this." Care:
    scroll velocity = "you're reading carefully, no interruption
    needed."
  - Engagement: returning visitors = retention metric. Care:
    returning to a section = "you wanted this back; surface
    related cards you've made before."

For our tool: the scroll-signals work filed under voice corpus
1b-β is the first piece of this. Its real purpose isn't just
"detect reading-paced for corpus prompting" — it's "use the
same signals analytics systems abuse, but route them to caring
behaviors instead of extractive ones."

This is a useful frame for evaluating future signal-collection
work: every new signal we collect should answer "how does this
serve the user?" rather than "how does this measure the user?"
If a signal can only answer the latter, we probably shouldn't
collect it. If a signal answers both, we route to the former.

## Right-margin overflow (v0.1.179 observation, exploratory)

User: "one thing that always bothered me about css and html and
what I see now in our engine is that the page slightly overflows
the right margin... this pattern is everywhere and I want to know
why and not let it happen here when it is messing with swipe
gestures as well, which should also have their own estimations
for these cases."

### Why this happens on the web at large

CSS allows authors to make content wider than the viewport in
many ways: fixed-pixel widths on tables/images, `white-space:
nowrap` text that exceeds container, `display: flex/inline-block`
children that don't auto-wrap, missing `box-sizing: border-box`,
desktop-designed sites without responsive breakpoints, framework
defaults that don't match actual viewports. Browsers tolerate
this by allowing horizontal scroll or by truncating with
`overflow-x: hidden` if author specified it. The web at large
treats right-overflow as a normal failure mode.

### Why we don't have to tolerate it

We control the destination format. Every Text widget can be
constrained. Every Image can be capped. Every Column can have
its width strictly bounded. There's no good reason for us to
inherit the CSS world's tolerance for overflow.

### Specific contributors fixed in v0.1.179

  - Text wrap mode was `Text.WordWrap` everywhere (13 instances).
    WordWrap doesn't break long unbroken words — URLs, code
    lines without spaces, slugs — these would extend past their
    `width` even though `width` was set. Changed all to
    `Text.WrapAtWordBoundaryOrAnywhere`. Now Text honors its
    width strictly: prefers word-boundary breaks, falls back
    to anywhere when a single word is too long. Bounded > pretty.

### Still to address

  - **Tables.** We render some tables as Row/Grid layouts.
    Need to audit whether fixed-width children can exceed
    parent. If so, cap each column or use a flickable Row.
  - **Inline form elements.** TextField has `width: parent.width
    - 20` which is OK, but other inputs might not.
  - **Image alt-text strings** longer than viewport. Already
    capped at 200 chars but at small font sizes might still
    extend. Need explicit `width: parent.width` on the alt
    Text emission (already there in some paths).
  - **Generated badges, pills, tag chips** with `width:
    childrenRect.width` or similar — these inherit the child's
    natural width and could overflow.

### Swipe-gesture implications

User noted right-overflow is "messing with swipe gestures."
Diagnosis: when content visually extends past `browseFlick`'s
`contentWidth`, edge-from-right swipes can be interpreted
ambiguously. The Flickable's `boundsBehavior` and the gesture
system may both claim the touch.

Filed but not yet built:
  - **Strict viewport-bounded contentWidth.** Ensure
    `browseFlick.contentWidth` is exactly viewport width, no
    drift. Already the case (`contentWidth: browseScroll.width`).
  - **Edge-gesture protected zones.** Reserve N pixels at the
    left and right edges of the viewport for back/forward
    swipe gestures. Touch events landing in those zones go to
    the gesture handler unconditionally, never to page
    content. Should be small (~16px) so it doesn't eat into
    usable content area.
  - **Swipe-gesture estimation.** User mentioned the gestures
    "should also have their own estimations for these cases."
    What does estimation mean here? Possibly: predict whether
    an in-progress swipe is a back-nav (mostly horizontal,
    starts near edge) vs a scroll (mostly vertical) vs a
    text-select (slow, both directions). The estimation can
    bias which handler wins based on early velocity and
    starting position. Filed as L: gesture-disambiguation
    framework.

## Page overflow — Row children claiming full width

- [x] ~~**Row children must not all claim parent.width.**~~
  Landed v0.1.417. CONFIRMED from a user QML capture: the
  renderer emitted `Row { width: parent.width ... Text { width:
  parent.width } Text { width: parent.width } }`. In a Qt Row,
  children lay out left-to-right at their own widths, so child
  2+ start at x≈parent.width and render OFF-SCREEN RIGHT — the
  "pages appear off to the side" bug (distinct from the
  v0.1.411 table-row line-count explosion). Fix: at container
  assembly, when layout is Row/RowWrap and >=2 child parts
  contain `width: parent.width`, demote to Column so they stack.
  Genuine horizontal layouts (toolbars, button groups) have
  small content-sized children that don't claim full width, so
  they're preserved. 2 tests. The durable solution remains the
  windowed-rendering arc below.

## Graph-view presentation batch (v0.1.446 session — user requests)

- [x] ~~**Native themed shapes for niftycharts.**~~ Landed v0.1.446:
  shape code threaded to the delegate; shape-aware radius + themed
  glyph badge.
- [ ] **Condensed graph / semantic sub-zoom.** User: "some sort of
  condensed graph or sub-zoom based on the last characters above."
  A focused view zoomed into a cluster (recent/top elements)
  rather than the whole sprawl.
- [ ] **Default zoom to first elements + minimap.** User: "default
  zoom to the first couple elements with a minimap of the general
  overview sidelined." Open framed on root + immediate children
  (legible landing, auto-presentation), small overview minimap in
  a corner for context + navigation. HIGH IMPACT for "holding
  their own" — needs the user's eyes on the feel.

## Graph-view polish batch (v0.1.445 session — user requests)

- [x] ~~**Nodes "hold their own" — weight + interactivity + auto-
  presentation.**~~ Landed v0.1.446: edge-degree importance
  weighting (fill + border), hover affordance (brighten/lift/
  cursor), hub breathing for auto-presentation. Addressed the
  user's "take up the screen without holding their own" critique.

- [x] ~~**Lerp on graph camera adjustments.**~~ DONE (verified v0.1.871;
  implemented across v0.1.413/717). Every programmatic camera move
  animates: `_fitToView` (scale via `_northScaleAnim` + pan via
  `_northFlickX/YAnim`), `_centerOnFocus` (pan via `_centerX/YAnim`
  SmoothedAnimation, hit from crumb taps through `_flashFocus`), and
  `_snapToNorth` (scale+pan+rotation). Interactive pinch/drag stay 1:1
  (correctly NOT lerped). No remaining programmatic snap.
- [ ] **Better touchpad snapping in graph view.** User: "add
  better snapping for touchpad graph view." Snap-to-node /
  snap-to-grid on pan release; tune with the lerp work.
- [ ] **Replay / go-to-page in graph view.** User: "replay or go
  to page in the graph view would be nice." Tapping a Page node
  → navigate the browser to that URL; possibly a "replay" that
  walks the session (ties to the existing session-walk).
- [x] ~~**Smart tags via NLP crawl suggestions.**~~ Landed
  v0.1.448. nlp::suggest_tags (TF-IDF + RAKE vs archive corpus,
  minus existing tags) → card_suggested_tags qt_method →
  tappable "+ tag" chips in the card preview. Also audited the
  existing NLP wiring (all connected; visibility gated by
  preconditions, not missing plumbing).

## Hardware findings (v0.1.444 session)

- [x] ~~**Mindmap render not useful (scattered boxes, literal \\n).**~~
  Fixed v0.1.444: proper indentation parser + label cleanup +
  title cap.
- [ ] **M — Touchpad drag + friction slings on the graph canvas.**
  User: "the touchpad needs drag and friction slings". The cards
  canvas Flickable has flickDeceleration but the touchpad
  drag→fling feel needs work. Tune flickDeceleration /
  maximumFlickVelocity / boundsBehavior; possibly add momentum.
- [~] **Audit: is the fast-NLP actually wired end to end?** User:
  "the NLP might not be fully wired up yet". Verify TextRank
  extractive → banner, RAKE smush, TF-IDF chips, SimilarContent
  edges, card_similar all actually fire on real pages (not just
  unit-tested). Likely needs the WEB2QML_LOG categories + a
  hardware pass; some may need qt_method call sites checked.
  - v0.1.862 call-site audit (verified by tracing pure fn → qt_method
    → QML consumer):
    - **card_similar** (SimilarContent): WIRED. qt_method consumed at
      qml/main.qml:23590 (card preview "similar cards").
    - **card_suggested_tags** (RAKE+TF-IDF via nlp::suggest_tags):
      WIRED. Consumed at qml/main.qml:23614 (card preview tags). So
      RAKE+TF-IDF DO reach the UI — through the card preview.
    - **TextRank extractive → banner**: NOT WIRED. extractive_summary
      (TextRank-backed) runs on page load (main.rs:14768/15010),
      stages, and is exposed via web_summary_text/web_summary_revision/
      session_summary_ready — but NO QML consumes any of them. The
      computed page summary dead-ends; there is no banner showing it.
    - **page_keywords** (standalone TF-IDF page chips): ~~DEAD qt_method~~
      REMOVED v0.1.870 (0 QML consumers; RAKE+TF-IDF already reach the UI
      via card_suggested_tags, so it was redundant). The shared nlp helpers
      it used (DocFreq, tfidf_keywords) stay — used elsewhere.
  - v0.1.862 also added `[nlp]` logging at the extractive stage
    (WEB2QML_LOG=nlp) to confirm it fires on real pages and whether it
    produces output vs empties.
  - REMAINING: (a) decide whether the extractive page summary should
    have a UI banner (wire web_summary_text + session_summary_ready
    into the web view) or is intentionally superseded by the sessions/
    LLM summary flow — a product-intent call, left for the user;
    (b) ~~remove or repurpose the dead page_keywords~~ DONE v0.1.870
    (removed). Item (a) is small once the intent is decided.
- [ ] **Helix latency: profile run pending.** `ollama ls` fast
  confirms the v0.1.441 probe is cheap and isolates helix from
  ollama. Run helix w/ WEB2QML_PROFILE=1 → read altscreen:enter
  vs pty:parse burst timing (v0.1.434 instrumentation).

## Logging (v0.1.439)

- [x] ~~**Category-routed logging + runtime flag.**~~ Landed
  v0.1.439. src/log.rs clog! macro; WEB2QML_LOG=all|cat,cat|off
  routes per-category files under ~/.config/web2qml/logs/.
  md/nav/pty converted as a start.
- [x] ~~**Incremental: migrate remaining `eprintln!("[cat] …")`
  to clog!**~~ DONE v0.1.870. Converted the full long tail (~44 sites
  across security, audit, moment, entity/dust/tree, data, audio_profile,
  connections, phonetic, phonetic_confusions, term_blocks/ssh-detect,
  pty_term, log_summary, token_log, missing_packages, fs_find, llm_cache,
  model_bench, path_fuzzy, qml_gen, separation, sudo_keepalive,
  voice_intent/floor-claim, voice_lang, web_summary, main.rs/profile).
  Behavior-preserving — `log::emit` echoes the same `[cat] …` line to
  stderr unconditionally and adds per-category file routing. Only the
  `eprintln!` in `log.rs` remains: it IS the sink (converting it would
  recurse). All `eprintln!("[cat] …")` log calls now route through clog!.

## Code cleanup (filed, low priority)

- [x] ~~**Consolidate epoch_secs/now_secs.**~~ Landed v0.1.454 (src/util.rs). Same
  unix-seconds helper duplicated in decision_cache, llm_cache,
  log_summary, missing_packages, system_info (i64!), term_blocks,
  token_stats, web_summary, workflow. Move to a shared util.
  NOTE: system_info returns i64, others u64 — not a pure
  find-replace. Do when cargo check is available between turns.

- [x] ~~**Consolidate FNV-1a (3 sites).**~~ Landed v0.1.454. connections::short_hash
  (→String), llm_cache::fnv1a, nlp::fnv1a share the same constants.
  A shared `util::fnv1a(&str)->u64` with thin wrappers. Low
  reward, multi-file; do alongside the epoch cleanup.

- [~] **Idle/pocket efficiency arc.** v0.1.459: app-state gate
  (wake-listener polls 4x slower backgrounded). v0.1.460: deferred
  batch-transcription policy module (warm-burst drain — pay whisper
  init once, amortize across clips). NEXT: wire enqueue(corpus)+
  drain; then unload whisper buffers on sustained background
  (~200MB) + reload-drains-queue on foreground/wake.


- [x] ~~**Dedup stopword lists + split_sentences.**~~ Done
  v0.1.438. 3 STOPWORDS arrays → nlp::STOPWORDS; 2 split_sentences
  → textrank::split_sentences.

## Fast NLP for summaries (v0.1.429 started)

- [x] ~~**TextRank extractive summaries.**~~ Landed v0.1.429.
  src/textrank.rs (PageRank over sentence-similarity graph, 7
  tests); web_summary::extractive_summary delegates to it. Picks
  central sentences, not first-N. No model → no timeout.

- [x] ~~**RAKE keyphrase extraction.**~~ Landed v0.1.430 in
  src/nlp.rs (rake_keyphrases). session_walk::word_smush now
  prefers RAKE phrases (freq fallback kept as word_smush_freq).
  Could also feed keyword chips in the summary banner — filed.

- [~] **TF-IDF keyword extraction.** Algorithm landed v0.1.430
  (nlp::tfidf_keywords + DocFreq). STILL TODO: persist + build
  the DocFreq table over the page archive so it runs WARM (not
  cold-start frequency); then expose the DF table / SparseVecs
  so the user's vectorizing project can read them. Deferred per
  user ("focus on summary quality first").

- [x] ~~**Hashing vectorizer for sentence/page similarity.**~~
  Landed v0.1.430 (nlp::SparseVec, FNV-1a hashing trick, cosine).
  Not yet WIRED into TextRank edges / dedup / "find similar" —
  those integrations filed below.

- [x] ~~**Build DocFreq table over the archive (warm TF-IDF).**~~
  Landed v0.1.431. DocFreq::from_texts builds from archived page
  snippets on demand (no separate file to drift); page_keywords()
  qt_method + banner keyword chips. STILL TODO: expose DocFreq +
  SparseVec for the user's vectorizing/pattern project (deferred
  per user — summary quality first).

- [x] ~~**Wire SparseVec into the graph + dedup.**~~ Done.
  v0.1.432: SimilarContent edge type (cross-domain topical
  links). v0.1.433: card_similar() + "Similar content" preview
  section (per-node find-similar) AND dedup_chunk_summaries
  (collapse near-duplicate chunk summaries via cosine >= 0.80
  before the combine pass). Remaining vectorizer work is just
  exposing the structures to the user's project (deferred).

- [ ] **Reactive collaborative command sessions (user's newer
  framing).** Distinct from passive room-summarization: when 3+
  people give rapid-fire feedback in the operator's terminal, a
  reactive session opens — the knowledge graph links what's
  discussed, responses can be saved, and suggested actions (e.g.
  power-users chaining commands) can be surfaced/run. User: "not
  sure about this one yet... come back to it." Design later;
  reuses nlp::speaker_digest. Less sensitive than room-summary
  since it's the operator's own collaborative session, but still
  wants a consent/attribution pass.

- [ ] **SENSITIVE — Multi-speaker comprehension aid: SOCIAL
  design before any live wiring.** NLP foundation landed v0.1.430
  (nlp::speaker_digest with confidence + MULTI_SPEAKER_MIN_VOICES).
  This feature affects human behavior (people summarized in a
  shared physical space), so the social layer is a PREREQUISITE,
  not an afterthought:
    - Consent: default to processing ONLY the operator's own
      captured audio for the operator's own comprehension; never
      silently profile other people in the room.
    - Misattribution: noisy multi-speaker attribution WILL err;
      the digest's `confidence` must gate display (show
      tentatively / withhold), never assert "X said Y" falsely.
    - Behavioral shaping: a shared live screen makes people
      perform/self-censor → favor an ambient aid private to the
      wearer over a shared display.
    - Activation: conservative default — auto-arm at 3+ voices
      but operator can disable; opt-in overall.
  Do NOT wire speaker_digest to live audio until this is settled
  with the user. Filed explicitly so it isn't built by momentum.

## Session walk (v0.1.426) — follow-ups

- [x] ~~**Prezi-style "walk me through my last session's pages."**~~
  Landed v0.1.426. session_walk.rs (last_session clustering +
  word_smush, 12 tests); session_walk_data qt_method; QML camera
  fly + pop/crossfade overlay + bottom scrubber + "Walk session"
  button.

- [~] **Voice trigger for the session walk.** PARTIAL
  v0.1.427: a mic on the scrubber + walk_advance_cue() handle
  the IN-WALK "keep going"/"vamos"/"dale" advance cue (no Intent
  variant needed). STILL TODO: starting the walk itself by voice
  ("walk me through my last session") as an Intent variant —
  that's the multi-file Intent enum + es/fr parsers + QML
  dispatch change; do when cargo check is available.

- [ ] **S — Walk: real page thumbnails / richer previews.**
  Node.thumbnail is None today; if page screenshots land, the
  walk overlay could show the actual page preview before the
  smush crossfade instead of re-showing the title.

- [ ] **S — Walk: session picker.** Today it walks only the most
  recent session. Could offer "walk session from <date>" by
  exposing earlier clusters.

## AI summary pathway — staged (v0.1.424 partial) + follow-ups

- [x] ~~**Staged web summaries (instant extractive → LLM upgrade).**~~
  Landed v0.1.424. extractive_summary shows the lede instantly;
  the LLM upgrades it or, on timeout (common on slow local
  models per the user's log), the extractive remains as fallback.

- [x] ~~**Chunked section summaries.**~~ Landed v0.1.425.
  chunk_text splits the page (~1500 chars/chunk, max 6) at
  sentence boundaries; each chunk gets a short one-sentence
  prompt (beats the per-call timeout where a big prompt didn't),
  staging a progressive partial after each so the banner builds
  up; a final combine pass merges them. Key locked per-call (not
  across the loop) to keep the decrypted-key window tight. Sets
  up per-chunk caching (item below).

- [x] ~~**Partial-summary cache for token reuse.**~~ Landed
  v0.1.440. The chunked-summary worker's run_one now does
  llm_cache lookup/store on every chunk/combine/single-page call
  — content-addressed (prompt embeds the page text), so a
  re-visit reuses prior work with zero tokens. ALSO did the
  voice fragment classifier (fragment_check_async) the same way —
  recurring short fragments resolve instantly with no ollama
  subprocess. (Disk-persisted via llm_cache's existing
  save_to_disk; the "intermediate steps" persist as their own
  prompts.)

## Open — need a runtime signal (NOT guessed at)

- [~] **S — Blank local markdown.** A local `.md` file renders
  blank. Should route via `.md` suffix -> manpages::to_qml.
  Need the `[md] routing ...` stderr line + temp QML to tell
  "didn't route" from "routed but empty/broken QML".
  - v0.1.857: pane rendering of local `.md` wired in build_page_qml
    (file:// .md → read → to_qml_with_source).
  - v0.1.860: the requested runtime signal is now in place on BOTH
    paths. Main-view path logs `[md] rendered … → N bytes QML`
    (flags <80 bytes as empty/broken) and `[md] did NOT route …
    (looks_like_html=…, head_len=…)` when a `.md` falls through.
    Pane path logs `[md] pane routed … (N in, M QML)` + read/stat
    failures. A blank render is now diagnosable from the log alone;
    reopen with the captured `[md]` line if it still renders blank.

- [ ] **S — Character rendering issues (apple.com).** Need a
  specific offending char + temp QML.

- [x] ~~**Snap-to-North touchpad sensitivity / graph glide.**~~
  Partially addressed v0.1.418: native Flickable flick physics
  (flickDeceleration/maxVelocity/pixelAligned) now drive
  two-finger trackpad swipes with proper coast; grab-pan got a
  velocity-tracking glide. If the snap-reset itself still wants
  to be more decisive, revisit with hardware feel.

- [x] ~~**Graph view scroll bounds (right/bottom unreachable).**~~
  Fixed v0.1.418: trailing margins on cardsFlick + margin-aware
  grab-pan clamp.

- [x] ~~**ollama ls slow vs konsole.**~~ Fixed v0.1.418: the grid
  had no DSR (Device Status Report) handler, so tools sending
  ESC[6n blocked for a cursor-position reply that never came.
  Added DSR 5/6 replies via a grid response_buf drained to the
  master each tick.

## Search bar enhancement (user direction)

- [x] ~~**Search bar: suggestions, actions, history.**~~ Landed
  v0.1.420. src/search_suggest.rs (pure, 10 tests) ranks history
  + bookmarks + path-fuzzy files + internal pages + a web-search
  action; backend.search_suggestions aggregates; QML
  urlSuggestPopup (overlay-parented, flips up for a bottom bar)
  shows the dropdown.

## Windowed rendering off an intermediate tree (v0.1.410 user direction — L/XL arc)

User observation (v0.1.410): a page generated **over 1,000,000
lines of QML** into /tmp and rendered far to the right. User's
proposed direction (correct): "taking a step towards keeping an
intermediate tree we have and loading only the section of the
page we need into qml."

### The problem

A 1M-line QML file is the disease; the off-to-the-side render
is almost certainly a symptom. `MAX_NODES=500` in qml_gen
SHOULD bound element emission, so 1M lines means EITHER:
  - a code path emits without going through
    `generate_safe_qml_with_stats`'s node counter, OR
  - per-element emission is exploding (1M / 500 ≈ 2000
    lines/element, implausibly high for normal elements →
    suggests a Repeater-with-huge-model or an unbounded inner
    loop somewhere)

### Immediate diagnostic (before the big arc)

NOT yet diagnosed — the user's QML/log pastes didn't reach the
dev context. To crack it, need from a hardware session:
  - the `[align] content overflow:` log line (v0.1.399
    diagnostic; prints childrenRect.x / width / loaderWidth /
    item.width — tells us if it's an x-offset vs an over-wide
    item)
  - `grep -c "Text {"` and `grep -c "Rectangle {"` on the temp
    file (if ≈500, per-element explosion; if ≫500, the node
    cap is being bypassed — find the path that emits without
    incrementing st.total_elements)
  - `grep -c "Repeater"` (a Repeater on a giant model would
    blow line count while staying few "nodes")

A targeted fix for whatever the diagnostic reveals is an S/M;
the arc below is the real, durable solution.

### The arc: virtualized/windowed rendering

Every real browser renders only the visible viewport + a
buffer, not the whole DOM. We should too:
  1. Parse the page into a lightweight intermediate tree held
     in the backend (we partly have this — the scraper DOM,
     but it's consumed per-render and thrown away).
  2. Emit QML only for the viewport + a scroll buffer.
  3. Regenerate the emitted window as the user scrolls (the
     browseFlick already reports contentY; hook it).

Benefits:
  - QML size bounded regardless of page complexity (kills the
    1M-line case)
  - Likely eliminates the overflow (smaller emitted subtree =
    fewer runaway-width chances)
  - Much faster first paint on low-perf hardware
  - Depth becomes "how deep we walk the INTERMEDITE tree,"
    cheap to change live (synergizes with v0.1.409's debounced
    depth re-render)

Size: L/XL, multi-ship. Needs careful design of the
tree-retention + scroll-windowing handshake. The single
biggest architectural win available for the rendering path.

## Graph view interactions (v0.1.410 user direction)

- [x] ~~**S/M — Single-click drag-to-pan on empty graph space.**~~
  Landed v0.1.412. Added a background `DragHandler` on an Item
  that's the first child of graphRoot (z:-1, beneath edges +
  cards). Snapshots cardsFlick.contentX/Y at grab, then drives
  them to start − activeTranslation×scale on each update —
  absolute (no drift). Cards' own drag MouseAreas
  (preventStealing, higher z) win on cards; this only engages
  on empty space. Used DragHandler over a hand-rolled MouseArea
  specifically to avoid the content-moves-under-pointer drift
  that bites a delta-tracker living inside the scrolled
  content. NOTE: feel/coordinate-frame is unverified on
  hardware — the standard DragHandler approach should be
  robust but the scale interaction wants an eyeball.

- [x] ~~**S — Tighter spacing tuning for highly-connected nodes.**~~
  Landed v0.1.412. The v0.1.399 density branch was BACKWARDS
  for the crowding symptom — it bumped attraction (×1.4) on
  dense graphs, which pulls connected nodes closer and tightens
  the hub. Fixed: dense graphs (density>1.5) now boost
  repulsion AND edge length (spread = 1+(density−1.5)×0.2,
  capped 1.5) so highly-connected hubs push neighbors out to a
  readable radius. Attraction nudge kept but gentled ×1.4→×1.1
  so edges still read without re-collapsing. Test updated to
  assert spread (dense graph has more repulsion + longer edges
  than a same-size sparse graph). Constants want a hardware
  eyeball to fine-tune.

## AI summaries: expand scope + scope-selector control (v0.1.410 user direction)

User direction: AI summaries (currently session/terminal
focused) should expand to the WEB as well — summarize the page
you're reading, not just terminal sessions. And the setting
should be a multi-option control.

- [x] ~~**M — AI summaries for web pages.**~~ Landed v0.1.414.
  New `src/web_summary.rs` with a focused page-summary path
  (separate from log_summary's terminal-block queue — shares
  only the LLM call). `summarize_current_page()` qt_method
  grabs the cached HTML, extracts readable text
  (`extract_page_text`), gates on `worth_summarizing` +
  in-flight, and spawns a worker calling
  `llm_provider::provider_query` (Ollama or Claude). Result
  stored in module state; QML polls `web_summary_revision()`
  (worker threads can't fire qt_signals — mirrors the
  `llm_poll_result` pattern). Auto-triggers from `update_nav`
  when scope is web-active; clears stale summaries on
  navigation. Floating collapsible/dismissable banner in the
  browser view shows the summary, gated to match the page on
  screen.

- [x] ~~**S — AI-summary scope selector: terminal/web/full/off.**~~
  Landed v0.1.414. `SummaryScope` enum {Off, Terminal, Web,
  Full} in web_summary.rs, persisted as `summary_scope=` in
  settings.conf (loader tuple grew 17→18, key-based so
  backward compatible). `summary_scope` qt_property +
  `summary_scope_set` qt_method drive both paths: the setter
  syncs log_summary's enabled flag from `terminal_active()`
  and the global SCOPE atomic from the whole value. QML: a
  4-pill segmented control (Off/Terminal/Web/Full) replaced
  the old on/off AI-summary toggle in settings; selecting
  Web/Full while on a page kicks an immediate summary.

## Image clamping philosophy (v0.1.180 user direction)

User: "we could try and paper over bad layout guesses by devs
with our own image sizes and keep them inline with compact vs
spacious setting... if someone wants to share about what pixels
are they can add a code block or some other way."

And: "we can also do rolling photos for those photos that are
just massive or zoom in tap to hold button on all photos added
if scale is more than 2x the screen in any dimension and clamp
it otherwise... unless the point is to have parallax which is a
different pattern and is more subtle design that can go a long
way if done right so maybe try to think of patterns that would
be 'right'."

### Philosophy

We don't have to inherit the web's image-size chaos. Authors
guess wrong about sizes; we paper over with opinionated defaults
that prioritize the reading experience. If someone *actually*
needs to communicate exact pixel dimensions, they can use a code
block or explicit dimension annotation — not via images.

### Implemented v0.1.180

  - Height cap at ~70% viewport (`_root._vpHeight * 0.7`)
    applied to all images, both `<img>` and `<picture>` paths.
    Images with width*aspect below the cap render at natural
    size; oversize images get capped, scaled down via
    PreserveAspectFit. No more 1500px portrait dominance.
  - `_vpHeight` plumbed from `browseFlick.height` via the
    pageLoader.onLoaded handler, same mechanism as `_vpTop` /
    `_vpBot`.

### Still to build

- **S — Compact / spacious user setting.** Toggle in settings
  mapping to height cap fraction. Compact = 0.5, default = 0.7,
  spacious = 0.85. Persisted in settings.conf. Single property
  read by the qml_gen Image emission via _root binding.

- **M — Rolling photos for >2x-viewport images.** When an
  image's intrinsic dimensions exceed 2x viewport in any axis,
  render it inside a horizontal Flickable that allows
  in-place pan. The user gets to see the full image at full
  detail without it dominating the page layout.

- **M — Tap-hold zoom-to-fullscreen for all images.** Long
  press on any image opens it in a fullscreen viewer at
  natural pixel size with pinch-zoom and pan. Composes with
  the tap-hold per-image action menu we filed (Save,
  Bookmark, Ask AI, plus Zoom). Same gesture, expanded menu.

- **M — Parallax pattern recognition.** Some image patterns
  *are* meant to be larger than viewport — background images
  with parallax effect, hero photos that fill the screen as
  the user scrolls. Detect these patterns (CSS background-
  image + transform: translateZ + scroll-linked transform; or
  certain class-name patterns like "parallax", "hero-bg") and
  exempt from height clamping. Without recognition, parallax
  pages render flat and lose their intended depth effect.
  Risk: misclassification — a non-parallax giant image gets
  exempted and dominates the page. Bias toward clamping
  (false negative on parallax) over exemption (false
  positive on regular).

- **L — Image gestures unified with text gestures.** The
  per-image tap-hold for action menu, plus pinch-zoom, plus
  scroll-pan inside rolling photos, plus the planned tap-hold
  on text excerpts (for capture-to-card) — all need a coherent
  gesture system. Filing a unified gesture-handler framework
  here. Composes with the swipe-gesture-disambiguation
  framework filed under right-margin overflow.

### Why this matters

Images are where the web's layout sloppiness most affects
reading. A tall image breaks scroll rhythm; a wide image
breaks viewport bounds; a parallax image renders flat. Each
needs a different intervention. The philosophy: assume the
author got it wrong; render for the user; provide escape
hatches (zoom, rolling) for the rare cases where the user
actually wants to engage with the image fully.

## URL resolution issues (v0.1.181 user report, diagnosed v0.1.182)

User v0.1.180 report: "url patterns were messed up somehow:
`Error: https://www.apple.com/ca/ca/apple-watch-ultra-3/:`"

Followup v0.1.181: "it was from sitemap"

### Diagnosis

The bug was in sitemap path storage, not link resolution.
Apple's sitemap.xml (or a sitemap reachable from robots.txt)
contains entries with doubled path segments like
`/ca/ca/apple-watch-ultra-3/`. Likely an Apple HTML-generation
quirk or a legacy redirect target that survives in the
sitemap. These got stored verbatim in our site map, then
reconstructed in `format_for_qml` as `https://{domain}{path}`,
producing the broken URL the user saw.

Also found a related bug: robots.txt `Sitemap:` directives
contain full URLs (e.g. `Sitemap: https://example.com/...`).
We were calling `add_from_robots(full_url)`, storing the URL
as if it were a path. format_for_qml then constructed
`https://{domain}{full_url}` =
`https://example.comhttps://example.com/...`. Never reported
by a user but a bug nonetheless.

### Fixes shipped v0.1.182

  - `is_safe_path()` validator that rejects empty paths, full
    URLs accidentally landing in path slots, paths not
    starting with `/`, and paths with consecutive duplicate
    segments. Logs to stderr when rejecting so we can see
    what's being filtered (and adjust if a legit pattern is
    being rejected).
  - `add_from_sitemap` and `add_from_robots` now both gate
    on `is_safe_path`.
  - `Sitemap:` directive handling in robots.txt parser
    changed from "store as path" to "skip" (with a TODO to
    actually fetch and parse the referenced sitemap).
  - Tests covering the bug cases.

### Still to address

- [x] ~~**M — Fetch and parse `Sitemap:` directive URLs.**~~
  Landed v0.1.227. `SiteMap` struct gained a `discovered_sitemaps:
  Vec<String>` field populated by `parse_robots_txt` from each
  `Sitemap:` directive. Caller (`site_discover` in main.rs)
  iterates the list, filters to same-domain via prefix match
  on `https://{dom}/` or `http://{dom}/`, caps at 5 URLs per
  session, and fetches each — skipping the default
  `/sitemap.xml` to avoid duplicate fetches with the
  always-tried default. Same-domain filter is deliberate:
  cross-domain Sitemap URLs in robots.txt are rare and
  following them is a security smell. Subdomain mismatches
  (apple.com vs www.apple.com) currently miss; can revisit
  if real sites trigger.

  Also detected and handled the **sitemap-index** vs **urlset**
  case: when a sitemap.xml file contains `<sitemapindex` (the
  nested-sitemap format), `parse_sitemap_xml` now warns and
  skips storing the `<loc>` entries (which would be sub-
  sitemap URLs, not user pages). Index-file recursion is
  filed below.

  8 new tests covering: directive collection (single + multiple),
  dedup of repeated directives, rejection of relative URLs,
  http/https acceptance, cross-domain preserved-but-caller-
  filters, sitemap-index skips, urlset still works.

- [x] ~~**M — Recurse into sitemap-index files.**~~ Landed
  v0.1.228. `parse_sitemap_xml` now returns the list of nested
  sitemap URLs when given a `<sitemapindex>`-flavored file
  (returns empty Vec for normal `<urlset>` content). The caller
  (`site_discover` in main.rs) feeds these to a new
  `fetch_nested_sitemaps` helper that iterates via worklist
  with same-domain filter, 10-URL cap per call, and depth-2
  bound (an index → leaf chain works; index → index → leaf
  stops at the second index without recursing further).
  Worklist-iterative rather than recursive because
  parse_sitemap_xml takes &mut self.site_maps and you can't
  recursively call a method that mutates &mut self from inside
  another &mut self method. The HashSet `seen` prevents
  double-fetch within a call. 2 new tests for nested-URL
  return value + dedup + absolute-only filter.

- [x] ~~**L — Cross-subdomain sitemap acceptance.**~~ Landed
  v0.1.400 (turned out S, not L). Was an exact-host prefix
  match that rejected the common apex↔www case (browsing
  apple.com, robots.txt lists `Sitemap:
  https://www.apple.com/sitemap.xml`). New
  `sitemap::same_registrable_domain(base, candidate)`
  accepts parent/child subdomain relationships (apex↔www,
  apex↔shop.apex) gated on a `.` boundary so look-alike
  hosts (notapple.com, evil-apple.com, apple.com.evil.com)
  still reject. Sibling subdomains (shop.x ↔ store.x) also
  reject — we don't compute the true registrable domain via
  a public-suffix list, just admit strict parent/child.
  Conservative by design since robots.txt is unauthenticated.
  14 unit tests covering acceptance + every security-relevant
  rejection. Wired into the robots.txt Sitemap: filter in
  main.rs via new `split_url_pub` host extractor.

- [x] ~~**M — URL resolution audit.**~~ Landed v0.1.401.
  Audited the sole URL-construction function
  (`qml_gen::resolve_url`, used by 8 call sites: link href,
  data-href, image src, picture source, video/audio src,
  form action). Found and fixed two real bugs: (1) `../`
  parent segments were left literal (e.g.
  `https://site.com/a/b/../images/x` instead of
  `https://site.com/a/images/x`); (2) a base with a query
  string but no path slash (`https://site.com?x=1`) plus a
  relative href produced garbage (`https:/page2`) because
  `rfind('/')` landed inside the `://`. Rewrote with proper
  origin/path decomposition, base query/fragment stripping,
  href-suffix preservation, and a `normalize_path` that
  collapses `.`/`..` (clamping at root like a browser). 21
  new unit tests covering every edge case the audit listed
  plus the two bugs. Hand-traced the trickiest cases since
  cargo isn't runnable in-sandbox. Confirmed common-case
  behavior (simple relative + absolute paths) is unchanged
  so existing pages don't regress. gemini.rs's
  `archive_path_for` was checked and intentionally left
  alone — it rejects `..` for path-traversal security in
  the disk cache, a different and correct concern.

## Speaker identity (v0.1.185 user observation)

User v0.1.184 report on speakers.toml: "no UUID? we might
want to get create this with public key that way naming we
can store sites and we can also change users names."

The point: speakers were keyed by `name` (mutable) as their
stable identifier. Renaming would orphan any references to
the old name. As we move toward cards/canvas where each
piece of captured content is attributed to a speaker, this
fragility becomes critical — a renamed speaker would
silently break attribution across months of accumulated
cards.

### Shipped v0.1.185

  - `Speaker` struct gained an `id: String` field. Format
    `spk_<unix_ms>_<seq3>` — readable, monotonic, no crypto
    dep. Stable for the lifetime of the entry.
  - `new_id()` generator.
  - load() backfills ids for older speakers.toml files
    lacking them, then saves back so migration runs once.
  - save() writes the id first in each speaker block.
  - New helpers: `list_with_ids()`, `rename_by_id()`,
    `lookup_id_by_name()`, `name_for_id()`.

### What stayed the same (intentionally)

The existing internal lookups (note_active, identify,
update_speaker, color_for, is_allowed, is_deep_muted, etc.)
still use name as the key. Migrating them to id is filed
below — the additive change in v0.1.185 doesn't require it.
Existing callers continue working; new code that needs
durability uses the id helpers.

### Still to address

- [ ] **M — Migrate internal speaker lookups to id-based.**
  Every `find(|s| s.name == name)` in speakers.rs becomes
  `find(|s| s.id == id)`. Voice path's identify() returns
  the id alongside the name (or instead of it). note_active
  takes an id. UI callbacks pass id around. Name becomes
  display-only. Substantial because ~15 call sites across
  speakers.rs and voice.rs, but mechanical.

- [ ] **L — Public-key-derived speaker identity.** The user
  raised this directly. Replace the `spk_<ms>_<seq>` format
  with a public-key fingerprint (e.g. base64 of an Ed25519
  public key derived from a per-speaker seed). Benefits:
    - Cryptographic provenance of cards: a card attributed
      to speaker X can be cryptographically signed by X's
      private key. Forgery resistance.
    - Cross-device identity: same speaker can have the same
      id on multiple devices if the keypair is the same.
      Federation between household devices becomes possible.
    - Selective sharing: speakers can sign cards they want
      to share externally without revealing identity beyond
      the public-key fingerprint.
  Adds infrastructure: keygen, key storage (encrypted at
  rest), signing primitives (ed25519-dalek or similar),
  serialization format. Worth doing only when we need at
  least one of those properties. For now, single-device
  use doesn't need it; the spk_<ms>_<seq> format is
  sufficient and forward-compatible (the field is
  `id: String`, format-agnostic).

- [ ] **M — Speaker id in cards.** When the cards substrate
  lands (post-voice-corpus arc), every card carries the
  speaker id of its author. Renames don't affect attribution.
  Queries filter by id. Display lookups go through
  `name_for_id` so deleted speakers don't break old cards
  (they show as "unknown speaker" rather than crashing).

## Cryptographic identity arc (v0.1.185 user direction, exploratory)

User v0.1.185: "proper setup gnupg like kleopatra in this app
would be good."

The right extension of the speaker-id work. Replace
`spk_<ms>_<seq>` opaque IDs with real cryptographic keypairs.
Cards get signed. Identity becomes verifiable. Cross-device
sync becomes possible without third-party servers.

### Why this fits

The historian's-record framing (signed cards, verifiable
attribution, durable across decades) needs cryptographic
identity to be real. Voice attribution is probabilistic;
PGP signatures are not. They compose: voice identifies
likely author; the user reviews and signs; the card carries
both signals.

Also: PGP is a standard. Cards signed today verify with any
PGP implementation in 30 years. Durability appropriate to a
lifetime knowledge graph.

### Why it's bigger than it sounds

Real PGP integration is substantial work:

  - **Primitives.** Use `sequoia-pgp` (full OpenPGP impl,
    multi-MB) for compatibility, OR use `ed25519-dalek` +
    hand-rolled packet writer for minimum dep, OR use raw
    ed25519 with our own format (not OpenPGP-compatible
    but simplest). Each choice trades off compat / size /
    work.

  - **Key generation.** Ed25519 is instant. RSA is slow on
    phone. Default ed25519.

  - **Passphrase management.** Encrypted private keys
    require entering a passphrase to sign. Auto-unlock for
    session is the right UX for personal device with
    "physical loss" threat model.

  - **Key storage.** `~/.config/web2qml/keys/<id>.{priv,pub}`,
    with backup story: write recovery seeds, optional
    encrypted-backup to multiple devices.

  - **Trust relationships.** Skip full PGP web-of-trust;
    use a simple "trusted speakers" list per device.
    Manual marking via QR exchange or fingerprint
    transcription.

  - **Key exchange.** QR code is probably cleanest.
    LAN-discovery via mDNS is fancier but more brittle.

  - **Revocation.** Signed revocation certs. Historical
    cards still verify against old keys; new cards use
    new keys.

  - **Verification UI.** Card display shows lock icon
    when signed-and-verified, broken icon when tampered,
    question mark when signed-but-unknown-key.

  - **Management UI (the "Kleopatra-inside" piece).** Per-
    speaker keypair view. Generate. Export. Import. Trust.
    Revoke. View signed cards.

### Sub-ship sequence (when we reach this arc)

  1. **Choose primitives + add the dep.** ed25519-dalek
     probably. Add minimum useful: generate, sign, verify.
     No UI yet.
  2. **Per-speaker keypair generation.** Speaker creation
     triggers keypair generation. Speaker.id becomes the
     fingerprint of the public key. Migration path for
     existing `spk_<ms>_<seq>` speakers.
  3. **Card signing primitive.** Card creation signs with
     the active speaker's private key. Card carries
     signature field. Load-time verification.
  4. **Verification UI in card display.** Lock icon /
     broken icon / unknown icon. Tap for details.
  5. **Key exchange via QR.** Two devices, two QR scans,
     mutual trust established.
  6. **Trust marking UI.** Browse trusted speakers,
     manually mark/unmark.
  7. **Revocation flow.** Generate revocation cert,
     publish to all known peers.
  8. **Kleopatra-style management UI.** All key operations
     in one place.

### Sequencing relative to other arcs

This arc **requires** the cards substrate (no point signing
nothing) and **prefers** working multi-voice attribution
(no point per-speaker keys if speakers aren't reliably
distinguished). Therefore:

  - Voice corpus arc completes first (5 ships).
  - Cards substrate arc completes second (~7 ships).
  - Cryptographic identity arc third (~8 sub-ships above).

This is a multi-month picture. The 1+8+8+8 ships at small-
to-medium each is realistic at one-ship-per-session pace.

### Forward compatibility

v0.1.185's speaker `id` field is `String` (format-agnostic).
When PGP lands, sub-ship 2 migrates `spk_<ms>_<seq>` IDs
to public-key fingerprints. Same field, new content. No
schema break.

Cards (when they land) will carry `author_id: String`
which is initially the `spk_*` value, later a key
fingerprint. Same field. Backward-compatible.

### What NOT to build prematurely

Don't add ed25519 keypair generation now. Don't add a
`pubkey` field to Speaker now. The work isn't useful until
cards exist to sign. Pre-building the field would be dead
weight. The right time is sub-ship 1 of this arc.

The architectural foresight that matters: `id: String` is
already format-agnostic. That's the only thing v0.1.185
needs to be.

## Paragraph selection follow-ups (v0.1.190 ship 2.1)

The mechanism is in place: tap a 10-50-word `<p>` → it gets
an accent strip → backend logs the selection. What's NOT
in 2.1, filed for next ships:

### Discoverability (ship 2.2 partial)

Currently nothing tells the user paragraphs are tappable.
The SoloPrompt "yes" flow should, in 2.2, show a brief
overlay saying "tap a paragraph to read aloud." Once the
user has selected one, the overlay can offer "start
reading?" with the WPM-paced highlighting.

### WPM-paced highlighting (ship 2.2)

Once a paragraph is selected, render an overlay that
progresses word-by-word at a default WPM (250 reasonable
default). Wraps the selected text with a per-word index;
a Timer advances the index every (60/WPM) seconds. Tap
to pause; tap again to resume.

### Recording (ship 2.3)

Tied to the highlighting: when WPM advance is running, the
microphone opens, audio captures to a buffer, saved on
completion to `~/.config/web2qml/corpus/<speaker>/<page>/
<timestamp>.wav` along with the selected text. Speaker is
attributed via the existing fingerprint mechanism.

### Known limitations of 2.1

- [ ] **L — Tap-on-invisible paragraphs.** RenderTier
  wraps top-level children in opacity-gated Items. A
  paragraph wrapper inside an opacity=0 tier wrapper still
  receives MouseArea events (Qt MouseArea isn't tied to
  opacity by default). Theoretically the user could tap a
  far-off-screen paragraph if they could somehow trigger
  it — they can't with normal touch, since the area isn't
  rendered. Not a real bug; file as L just in case.

- [ ] **M — Inline-link flattening.** The renderer already
  flattens `<a>` inside `<p>` to plain text (existing
  behavior, not ship-2.1 introduced). With paragraph
  selection, this means links in selected paragraphs
  can't be followed by tapping them — the paragraph
  swallows the tap. Restoring inline link rendering AND
  preserving paragraph selection would require either:
    - Excluding paragraphs with `<a>` children from the
      selection wrapper, OR
    - Two-stage interaction: short tap selects paragraph,
      long-press or modifier-tap follows link.
  Defer until users complain.

- [x] ~~**M — Paragraph count metric.**~~ Landed v0.1.199.
  `FilterStats.selectable_paragraphs` increments per wrap;
  logged at end of generation when non-zero. Useful for
  diagnosing "why is the read-aloud feature missing on this
  page" without a debugger.

- [x] ~~**L — Tunable word-count bounds.**~~ Landed v0.1.201.
  Env-var overrides `WEB2QML_PARAGRAPH_MIN` and
  `WEB2QML_PARAGRAPH_MAX` read once at startup via OnceLock.
  Defaults 10..=50. Logs the override at first use so the
  user knows it took effect. Invalid (min > max) reverts to
  defaults with warning.

## Dynamic tables follow-ups (v0.1.197 ship 2.4.2)

Ship 2.4.2 wired TableData into the renderer with viewport-
aware mode switching (wide grid above 600px, stacked cards
below). The remaining arc:

- [x] ~~**2.4.4 — Interaction surface for tables.**~~ Landed v0.1.198.
  table_nlp wired into a live command bar. Per-table state in a
  global qml_gen `TABLE_VIEWS` Mutex registry (chose this over
  Backend HashMap to avoid threading state through 5
  generate_safe_qml call sites). Re-render via existing
  `render_cached(url)` path — no history mutation, no state clear.
  Multi-table dispatch via `table N:` prefix; single-table omits.
  v0.1.200 added `undo` / `reset` meta-commands typed in the bar.

- [ ] **2.4.5 — Edit mode polish.** AddColumn / FillColumn
  work via NLP today but no UI affordance. Touch-driven
  add/edit (long-press column header for menu).

### Limitations of 2.4.2 as shipped

- [x] ~~**Large tables double QML size.**~~ Landed v0.1.421.
  Above TABLE_LOADER_CELL_THRESHOLD (100 cells), each mode wraps
  in a Component+Loader gated to its visibility (active:!_narrow
  / active:_narrow), so only the visible mode instantiates —
  halving the realized element count. Small tables keep the
  inline both-modes form for snappier switching.

- [x] ~~**M — Markdown tables in MANPAGE markdown parser.**~~
  Landed v0.1.214 — different scope than the originally-filed
  fetch-pipeline integration (see filing below). The manpage
  markdown parser (manpages.rs's `to_html`) now detects pipe-
  delimited rows with a separator line and routes through
  `table_ops::parse_markdown_table`, emitting HTML `<table>`
  that the existing adaptive-table renderer in qml_gen picks
  up. 10 new tests covering the parser detection and HTML
  emission. The welcome manpage has a quick-reference table
  as a smoke test.

- [x] ~~**M — Markdown tables in HTTP fetch pipeline.**~~
  Landed v0.1.408 (as content-type-driven markdown routing,
  which subsumes the table case — tables are just one
  markdown construct the manpage renderer already handles).
  The fetch now sniffs the `Content-Type` header on the
  primary success response (lowercased, params stripped) and
  stores it in `_last_content_type`. The existing markdown
  detection block (v0.1.265, previously URL-suffix-only) now
  ALSO fires when the content-type is `text/markdown` or
  `text/x-markdown` — so a site serving raw markdown at a URL
  WITHOUT a `.md` suffix routes through `manpages::to_qml`
  (headings, code fences, tables, paragraph selection) instead
  of being mangled as HTML. Guarded: the content-type path
  still requires the body to not look like an HTML document
  AND to have a markdown-ish opening, so a mislabeled HTML or
  JSON blob isn't force-rendered. Primary-path only (the
  retry/403 arms are for blocked HTML, not markdown servers).

- [ ] **L — Threshold is hard-coded.** 600px is a
  reasonable phone/tablet boundary. Could be a
  RenderStyle field if user testing shows different
  natural breaks.

- [x] ~~**M — Header truncation in narrow mode.**~~ Landed v0.1.200.
  Added `elide: Text.ElideRight` on the header label Text so
  long names like "Subscription Renewal Date" cut to
  "Subscription Re…" instead of wrapping to 3 lines and
  breaking card alignment.

- [ ] **L — Tap to switch column visibility.** A long-
  press on a column header in wide mode could show
  options "hide / move / filter." Hooks into 2.4.4.

## Manpage parser limits (v0.1.220 follow-ups)

The v0.1.220 ship handled italic, wikilinks, backslash escape,
and image placeholders + made table cells preserve inline
markup. Several more formatting variations were observed in
user-supplied markdown (a course-notes file with rich
formatting) that the parser still doesn't handle:

- [x] ~~**M — Escaped pipes in table cells.**~~ Landed v0.1.229.
  `split_md_row` now walks chars with backslash-counting logic
  per the GFM spec: count consecutive `\` before each `|`; odd
  count means the last backslash escapes the pipe (consumed,
  pipe becomes literal cell content); even count means the
  pipe is a cell delimiter (backslashes pass through verbatim
  to the inline parser). Trailing pipe stripping is also
  escape-aware (an `\|` at end-of-line stays as cell content).
  Multi-byte UTF-8 handled via char-iteration. 6 new tests
  covering single-escape, double-backslash, triple-backslash,
  trailing-pipe escape preservation, regression on bare-pipe
  splitting, and emoji-cell sanity.

- [x] **M — Mermaid code blocks.** **Landed v0.1.330.**
  Markdown `` ```mermaid `` fenced blocks render as code
  AS BEFORE (so the source is still readable) PLUS an
  inline "View as diagram" link beneath. Tapping the link
  decodes the percent-encoded source and opens the cards
  canvas in mermaid mode. Backend's `nav` intercepts
  `web2qml://mermaid?src=...` URLs, decodes, and emits
  `mermaid_source_clicked(source)`. QML Connections
  listens and calls `cardsCanvas.openMermaid(source)`.
  `scripts/qml_lint.py` extended with a third check that
  catches the empty-Component pattern that caused the
  v0.1.329 runtime load failure (see changelog). 7 new
  tests cover percent-encoding + fence detection.

- [x] **S — Strikethrough `~~text~~` (GFM).** Landed
  v0.1.402. Added to all three inline paths:
  `inline_to_richtext` (QML, emits `<s>`), `render_inline`
  (HTML, emits semantic `<del>`), and `strip_inline_markup`
  (read-aloud/titles, strips delimiters keeps body). Uses
  the existing general `detect_emphasis(bytes, i, b'~', 2)`
  run-delimiter matcher — same machinery as bold, just a
  different delimiter byte. Single `~` (e.g. "~5 minutes")
  correctly stays literal; double `~~` required per GFM.
  Nested markup inside strikethrough resolves recursively.
  7 new tests across the three paths.

- [ ] **L — Headings inside list items.** Some markdown
  documents have `### sub-heading` indented under a `-` bullet.
  The current list-gathering loop breaks on the heading (which
  is correct CommonMark) so the heading gets emitted separately
  at the top level. The user might expect the heading to render
  as a styled in-list label. Defer — semantics aren't clear.

- [ ] **L — Tables inside list items.** A `-` bullet whose
  continuation includes a `| col | col |` table currently gets
  the table swept into the bullet's text via the list's
  continuation-line accumulator. The pipe-table detector runs at
  the top of the main parse loop, not inside the list-item
  inner loop. Fixing it means making the list continuation
  loop also detect table boundaries. Defer pending real usage.

- [x] ~~**L — Block-level images.**~~ Landed v0.1.222, expanded
  v0.1.233. A markdown line that is purely `![alt](src)` becomes
  a bordered card (in QML) or a top-level `<img>` (in HTML).
  
  v0.1.222 emitted ONLY a placeholder card (camera + alt + src in
  monospace) because we couldn't trust relative paths to resolve
  in the bundled-manpage context. User feedback ("our new image
  custom thing is not drawing images") motivated v0.1.233's
  upgrade: when `src` is loadable (http://, https://, file://,
  or data:image/), emit a real QML `Image` element with async
  fetch + aspect-preserving fill. Italic muted caption beneath
  shows 📷 + alt. When `src` is relative or empty (the
  placeholder case), keep the prior v0.1.222 fallback. 5 new
  tests covering loadable URLs, file scheme, data URLs,
  non-image data URLs (still placeholder), and empty-alt no-caption.
  
  Inline images (images embedded mid-paragraph) still get the
  "📷 alt" placeholder; rendering inline Images would require
  splitting the paragraph Text element which is hairier and
  rare in real docs. Filed below.

- [ ] **L — Inline images render as real Image elements.**
  v0.1.233 made BLOCK images render for absolute URLs. Inline
  images (`Some text ![icon](https://...) more text`) still
  show "📷 icon" placeholder inline. Real inline rendering
  would need: (a) split the paragraph Text at the image
  position, (b) emit a row of [text-before, Image, text-after],
  (c) preserve word-wrap across the boundary. Complicated;
  defer until a real document needs it.

- [x] ~~**M — Code block id collision in manpages.**~~ Landed
  v0.1.223. `emit_code_block_qml` now derives a unique inner-Text
  id from `next_paragraph_id()`, same pattern as
  `emit_block_image_qml`. Outer Rectangle's
  `height: _code_<N>.implicitHeight` references the matching
  inner declaration. 3 new tests including a regression guard
  that asserts no naked `id: _code` leaks and a structural check
  that every outer-id reference has a matching inner declaration.
  Also caught + fixed a v0.1.222 test bug along the way
  (`block_image_emits_card_in_qml` had `\\\"monospace\\\"` where
  the actual output uses plain `"monospace"`).

- [x] **L → S — Triple-asterisk bold+italic.** **Landed v0.1.286.**
  `***foo***` and `___foo___` now render as bold+italic
  (HTML: `<strong><em>...</em></strong>`, Qt RichText:
  `<b><i>...</i></b>`). Implemented by calling
  `detect_emphasis(..., 3)` BEFORE the count=2 (bold) check
  in all three inline parser paths (HTML emit, Qt rich-text,
  plain-text strip). The existing "next byte must not be
  same delim" guard correctly rejects ambiguous 4-open cases.
  Original L scoping was conservative; turned out simpler
  than feared because the v0.1.221 `detect_emphasis`
  refactor already supported variable count parameters.

- [x] ~~**L — Emphasis with `_`.**~~ Landed v0.1.221. CommonMark
  `_italic_` and `__bold__` now render as `<em>` and `<strong>`
  (HTML) / `<i>` and `<b>` (Qt rich-text). Intraword guard
  applied: `foo_bar_baz` stays literal (the `_` is preceded by
  alnum). Star delimiters keep their no-guard behavior (`a*b*c`
  still emphasizes "b"). Generalized via a new `detect_emphasis`
  helper that takes delimiter byte + run length, replacing the
  three inline-parser ad-hoc bold detectors. 12 new tests.

## Connections overview arc (user request, v0.1.196)

Obsidian-inspired graph view of voice/file/web/chat nodes.
Sub-ships sequenced; **2.5.0 landed in v0.1.196 as data spine.**

- [x] ~~**2.5.0 — Node graph data model.**~~ `src/connections.rs`
  with `Node`, `NodeKind`, `Edge`, `EdgeReason`, `GraphSnapshot`.
  Scans existing data sources (voice corpus, workflows, speakers)
  + `attach_archive` / `attach_chat` for the Backend-owned
  systems. Derives edges from metadata overlap (SameDomain,
  VoiceBySpeaker, SharedTag). 11 tests pinning down node
  uniqueness, edge derivation, ISO 8601 parsing, FNV hashing.

- [ ] **2.5.1 — Voice recordings tagged with source page.**
  When `corpus_recorder::start_recording` fires from the
  reading flow, capture the current `backend.current_url` and
  page title alongside the paragraph text. Store as a third
  sidecar file or extend the `.txt` to include URL+title
  metadata. Enables the `VoiceFromPage` edge variant to be
  derived in `connections::derive_edges` (currently it's
  defined but never produced).

- [x] **2.5.2 — Fuzzy filter substrate.** **Landed v0.1.323.**
  `connections::fuzzy_filter(snap, query, cap) -> Vec<usize>`
  scores nodes by: whole-query substring of title (+100),
  whole-query substring of haystack (+50), per-token prefix
  of haystack word (+20), per-token substring of haystack
  (+5). Returns indices sorted by score desc, zero-score
  filtered out. `Backend::cards_filter` exposes it; QML has
  a live search field above the canvas; non-matching cards
  fade to opacity 0.18 and edges between unmatched endpoints
  fade. 9 unit tests.

- [x] **2.5.3 — User-assigned tags + sidecar persistence.**
  **Landed v0.1.324.** New `src/user_tags.rs` module:
  `HashMap<id, Vec<tag>>` persisted to
  `~/.config/web2qml/user_tags.toml`. Tags normalized
  (lowercase, spaces→dashes, punctuation stripped). 13
  unit tests covering normalization edge cases, add/remove
  roundtrip, dedup, nodes-with-tag lookup. New
  `connections::merge_user_tags(&mut snap)` appends user
  tags to each node's auto-derived tags then re-runs
  `derive_edges` so user tags participate in SharedTag
  edges. Wired into all 4 Backend snapshot-build call
  sites. QML preview overlay has separate user-tag chip
  row (accent border, × delete) and inline "+ Tag" input.

- [x] **2.5.4 — Voice command integration.** **Landed
  v0.1.319 + v0.1.323 + v0.1.324.** All five originally
  envisioned intents now exist:
  - "open connections view" → v0.1.319
  - "filter to <query>" / "clear filter" → v0.1.323
  - "filter to @<tag>" / "show only tag <X>" → v0.1.324
  - "tag this <X>" / "untag <X>" → v0.1.324
  - "go to <node>" — STILL DEFERRED. Less essential now
    that tap-to-preview + filter-fade-to-find covers the
    common navigation use case. Will revisit if user asks.

- [x] **2.5.5 — Graph layout engine.** **Landed.** Foundation
  in `graph_layout.rs` (v0.1.317): force-directed FR with
  rectangle-aware repulsion, pinned-node support, center
  gravity, 12 tests. Backend glue in v0.1.318
  (`Backend::cards_layout()` enumerates + lays out + serializes).
  Pin persistence in v0.1.321 (`~/.config/web2qml/card_pins.txt`).

- [x] **2.5.6 — Spatial navigation (QML).** **Largely landed
  through v0.1.319 + v0.1.320 + v0.1.322.**
  - v0.1.319: pan/zoom canvas Popup; opens via "show
    connections" / "show graph" voice intents; Flickable
    pan + PinchHandler zoom (0.3..3.0); kind-tinted cards;
    Canvas edges.
  - v0.1.320: drag-with-reflow. Tap-and-drag any card; on
    release, pin commits to backend and layout re-runs so
    the rest reflows around it.
  - v0.1.322: tap-to-preview overlay (bottom panel, kind
    chip + title + subtitle + preview text + tags + Unpin
    / Open source / Close).
  
  Remaining (sub-ships in this list separately): tap-and-
  hold on web tab as ENTRY GESTURE (currently entry is
  voice-only); filtered-fade visual when search arrives.

- [x] **2.5.7 — Edge weights + graph algorithms.** **Landed
  v0.1.325.** `EdgeReason::weight()` calibrated (VoiceFromPage=4,
  VoiceBySpeaker/SharedTag/ChatReferencesPage=3, SameDomain=1).
  `connections::related_to(snap, id, n) -> Vec<(id, weight)>`
  with undirected scan + sum-of-parallel-edges + sorted by
  total weight desc. `connections::pair_weight(snap, a, b)`
  for visual thickness. `Backend::card_related(id, n)` qt_method.
  Preview overlay gets a "Related to" surface (up to 5 rows,
  kind dot + title + weight chip, tap to switch preview).
  Cards canvas serialization now bundles edge weights;
  Canvas paint uses `sqrt(w) * 1.2` clamped to [1, 5] for
  line thickness. 10 unit tests.

## Markdown editor + git versioning arc (user request, v0.1.196)

User asked for an "insert math or table block" button when in
markdown, with input, write-to-file + commit history saved to
app cache + optional auto-temp-branch in git if repo nearby.

**Big prerequisite**: there's no markdown editor surface in the
app today. The browser renders pages. To do this, we first
need an editor. That's a substantial new feature, so sub-shipped:

- [ ] **2.6.0 — Markdown editor surface.** A new view (or
  panel within the browser) that opens an editable markdown
  file. Probably a `TextArea` + a preview toggle. File path
  source: argv, a "files" menu, or follow-tab-on-local-file
  from a future file-tracker.

- [ ] **2.6.1 — Insert math block button.** When editing
  markdown, a small toolbar (tap-and-hold to reveal?) with
  "insert math block" / "insert table block" affordances.
  Inserts the block scaffold at cursor + opens an input
  modal scoped to that block (LaTeX for math, table
  description for the table_nlp parser).

- [ ] **2.6.2 — Write-to-file commit history.** Each save
  appends a snapshot to `~/.config/web2qml/edits/<file-hash>.history`.
  Bounded ring (last 50 saves) so cache doesn't grow without
  bound. Restore-from-history via UI.

- [ ] **2.6.3 — Optional git integration.** On save: detect
  if the file is inside a `git` repo (walk up for `.git`).
  If yes, AND total file size under threshold (say 100KB), AND
  not on `main`/`master`, optionally create a `web2qml/<ts>`
  temp branch and commit. User confirms first (no silent
  history writes). Settings toggle controls auto-confirm.

- [ ] **2.6.4 — Block-level previews.** Math and table blocks
  render inline below their source as the user edits. For
  math: MathJax-style hint (probably won't fully render in
  v1; just verify well-formedness). For tables: live preview
  via the v0.1.195 `parse_markdown_table` + `to_markdown`.

## Mermaid plus renderer arc (user request, v0.1.196)

User asked for a Mermaid renderer with anchor points and an
"adapted layout engine to handle partial constraints in
multi-object relative positioning." This is essentially
designing a small diagram engine. Multi-ship arc:

**v0.1.317 update**: started the arc by landing the
**layout engine foundation** (`graph_layout.rs`). Parser
arrives in a later ship; the layout engine is what 2.7.2
+ 2.5.5 + 2.5.6 all need anyway.

- [x] **2.7.0 — Mermaid parser (subset).** **Landed
  v0.1.326.** `src/mermaid.rs` parses graph TD/LR/TB/BT/RL
  + flowchart alias, %% comments (line + trailing), node
  shapes (rectangle, rounded, circle, diamond, parallelogram
  L/R, asymmetric), edge styles (arrow, line, thick, dotted)
  with both pipe-form (`A -->|lbl| B`) and dash-form
  (`A -- lbl --> B`) labels, chained edges (`A --> B --> C`),
  implicit node declarations, explicit-overrides-implicit.
  Unrecognized lines captured in `graph.unrecognized` for
  diagnostics rather than fatal. 27 unit tests.

- [ ] **2.7.1 — Anchor point model.** Each node has anchor
  candidates (N/S/E/W + corners). Edges connect specific
  anchors. The "plus" in the user's request likely means
  extended Mermaid syntax for explicit anchor selection
  (e.g. `A.east -- B.west`).

- [x] **2.7.2-foundation — Force-directed layout engine.**
  **Landed v0.1.317.** `src/graph_layout.rs` with
  `CardPos`, `LayoutEdge`, `LayoutConfig`, and
  `layout_force_directed` + `seed_grid` + `bounding_box`.
  Rectangle-aware repulsion (cards don't overlap at rest),
  pinned-node support (the "partial constraint" the user
  asked for), center-gravity (disconnected components
  don't drift). 12 tests. ~340 lines. Parser-agnostic —
  consumed by both connections-view and future Mermaid+
  renderer. Full constraint-solving (Cassowary-style) is
  deferred.

- [ ] **2.7.2-full — Constraint-system layer.** Build atop
  v0.1.317's force-directed base when explicit alignment
  becomes a user need ("A and B horizontal at y=100",
  "subgraph X contains Y, Z"). Solver: simplex-like over
  hard constraints, fall back to forces for soft prefs.

- [x] **2.7.3 — QML rendering of parsed Mermaid diagrams.**
  **Landed v0.1.327.** Backend `mermaid_to_layout(source)`
  emits NODES/EDGES blob with shape-aware kind mapping +
  style-aware weight; cards canvas extended with
  `_sourceMode` switch ("connections"|"mermaid") via
  `openMermaid(src)` / `openConnections()`; edge canvas
  paint upgraded with arrowheads (filled triangle, sized
  by weight), dashed strokes for dotted edges, thick
  override for `==>`, edge labels rendered at midpoint
  with themed pill background, line termination pulled
  back from card centroids by half-extent so arrowheads
  sit inside the perimeter. Voice intent "show mermaid
  demo" opens canvas with a stock diagram demonstrating
  all node shapes + edge styles.

- [ ] **2.7.4 — Editing affordances.** Drag nodes to
  re-anchor pinned positions. Edit text inline. Re-run
  layout on each change.

- [ ] **2.7.5 — Round-trip.** Edits in the rendered view
  serialize back to Mermaid+ source so users can share
  the text and re-import.

## Multi-user onboarding arc (user direction, v0.1.202)

User wants to test the onboarding/registration flow as a feature.
The vision spans five substrates. v0.1.202 landed E (settings
tap/hold + quick popup). The remaining four are sequenced below.

User's design constraints:
- State coordination is "simulate user navigation through existing
  pages with delays + highlight animations" — NOT a stack of
  modal pages. New text overlays are thought bubbles.
- Onboarding doc lives "like man pages" — a doc store with named
  entries, lookup via something resembling `man <topic>`.
- Onboarding reading happens in a regular browser tab; user reads
  the markdown aloud paragraph by paragraph using existing ship-2
  selection + capture; backend + git + whisper figure out the
  profile in the background; bottom notifier signals "ready."

Ships in dependency order:

- [x] ~~**2.6.1 — Man-page system (substrate A).**~~ Landed v0.1.203.
  `src/manpages.rs` (~480 lines + 20 tests) with compile-time
  registry via `include_str!`, lookup by name, and a tiny
  markdown-to-HTML converter (headings, paragraphs, lists,
  links, inline+block code, bold). URL scheme `web2qml://man/<name>`
  routes through load_page's new arm. Two pages shipped: `welcome`
  (onboarding stub) and `manpages` (index). Bare `web2qml://man`
  or `web2qml://man/` redirects to the index. Unknown name shows
  a not-found page with a link back to the index. Nice composition
  side-effect: ship-2's paragraph selection works on manpages
  automatically since they flow through `generate_safe_qml`.

  - [ ] **M — Upgrade markdown parser.** The v1 converter handles
    a deliberately small subset (headings, paragraphs, lists,
    links, code, bold). For richer onboarding content (tables,
    blockquotes, emphasis, images, nested lists) swap in
    pulldown-cmark. The interface `to_html(md: &str) -> String`
    won't change. ~200KB binary impact.

- [x] ~~**2.6.2 — Thought bubble overlay (substrate B).**~~ Landed
  v0.1.204. `ThoughtBubble` QML component with `target: Item`,
  `text: string`, `dismissOnTap: bool`, `acknowledged` signal,
  and imperative `show()`/`hide()` methods. Positions itself
  below target with a tail pointing up; clamps to viewport
  edges; tail re-aligns to point at target center even when
  bubble is shifted. Fade in/out on show/hide. Single root-
  level instance `globalThoughtBubble` for the rest of the
  app to use. Demo trigger in quickSettingsPopup ("show bubble
  demo") points at the back button — testable end-to-end.
  Limits in v1: below-target placement only (no flip), one
  bubble at a time (no stacking), text-only content.

- [x] ~~**2.6.3 — Guided tour driver (substrate C).**~~ Landed
  v0.1.210. `tourDriver` QtObject in main.qml with `start(script)`,
  `stop()`, and convenience `startIntroTour()`. Script format: JS
  array of `{type, ...}` step objects. Supported types: `bubble`
  (acks via globalThoughtBubble.acknowledged), `navigate` (calls
  backend.nav), `delay` (Timer-driven), `jobStatus` (set
  notifier), `clearJob` (dismiss notifier). Target resolution via
  string-id → Item lookup in `_resolveTarget` (currently knows
  `backBtn` and `settingsBtn`). Built-in intro tour: navigate to
  welcome manpage → bubble on back button → bubble on settings
  gear → job-status terminal "ready" beat → clear. Tour is
  cancellable via stop(); bubble demo trigger checks for active
  tour and stops it before showing the standalone bubble.
  Composes A (manpages), B (thought bubbles), D (job notifier)
  into a sequenced experience.

- [x] ~~**2.6.4 — Background job progress notifier (substrate D).**~~
  Landed v0.1.209. Backend `job_status: QString` + `job_progress: f32`
  qt_properties driven by `set_job_status(text, progress)` and
  `clear_job_status()` qt_methods. QML thin strip at the bottom of
  the window auto-shows when status is non-empty, auto-hides 500ms
  after clearing. Spinner during active progress, check mark when
  text starts with "ready" or progress >= 0.999. Progress bar visible
  only during active progress (0.001 < p < 0.999). Reusable for any
  background work — the onboarding flow in 2.6.5 will be the first
  real consumer.

- [x] ~~**2.6.5 — Onboarding flow wiring.**~~ Landed v0.1.212.
  `tourDriver.startOnboarding()` composes A+B+C+D: navigates to
  `web2qml://man/onboarding` (new manpage with reading-aloud
  content), shows a bubble explaining the next step, sets
  `root.onboardingActive = true`. User then taps the voice button
  and reads the paragraph; the voice popup's completion path
  checks onboardingActive and routes to `speakerRegistrationPopup`
  instead of normal intent dispatch. The dialog shows a color
  swatch derived from the captured audio via new qt_method
  `speaker_preview_color()`, plus a name TextInput defaulting to
  "Me". Save calls `speaker_register_current(name)`, fires job
  notifier "Voice profile ready!", auto-clears after 2s. Cancel
  drops onboardingActive and closes without commit.

  Entry from quick-settings ("register your voice" link) and
  available as `tourDriver.startOnboarding()` for future automatic
  first-launch triggering.

  Honest limits (per arc completion):
  - Single capture only (not multi-paragraph blending). The
    `speakers::add_speaker` averages across calls anyway, so
    additional voice usage continues to refine the profile.
  - Color is derived from a single utterance — the original
    filing wanted accept/override with a slider. v1 has no
    override UI; the swatch is preview-only. Filed L for
    follow-up override slider if it becomes desired.
  - No first-launch auto-trigger. User must invoke from
    quick-settings. Filed L.
  - tourDriver.stop() doesn't reset onboardingActive; user
    must use the dialog's Cancel button. Filed M.

- [x] ~~**2.6.6 — MFCC → RGB color suggestion.**~~ Landed v0.1.211.
  `color_from_profile(profile, fallback_seed) -> String` in
  speakers.rs derives a per-speaker hex color from TonalProfile:
  pitch_hz → hue (0..360 with 80..350Hz mapping), voicing_prob →
  saturation (0.50..0.75), fixed lightness 0.55. Empty/no-signal
  profiles (pitch < 50) fall back to DEFAULT_COLORS palette via
  the fallback_seed. add_speaker uses it for new speakers; existing
  speakers keep their pre-existing colors (no migration). 10 unit
  tests cover hex format, stability, pitch-changes-hue, voicing-
  affects-saturation, extreme-pitch clamping, and HSL conversion
  primary-color sanity. Original filing called for richer mapping
  (orange-purple for volume, green for cadence); held off — the
  simpler pitch→hue + voicing→saturation lands the visual
  distinguishability without the eye-bleed risk. Future ships can
  layer in cadence/volume overlays if speakers turn out to be too
  similar in pitch.

## Voice-profile synthetic-bootstrap arc (user direction, v0.1.225)

User raised this idea in the v0.1.225 session as a way to reach
useful speaker matching faster than waiting for organic voice
samples to accumulate. Verbatim:

> "we can use TTS to synthesize a voice range for the full
>  paragraph immediately to start building up matches with
>  pitch, compression, distortion, gaps, stutters, crossfades,
>  etc. to create text merges or text equivalents... we can
>  blend with white and pink and brown noises for extra samples
>  ... we can build out dumb prints and train a small neural
>  model as well"

The core insight: every paragraph in a manpage IS ground-truth
text. We have the text the user IS GOING TO READ. We can
synthesize that text via TTS BEFORE the user speaks it, with
deliberate parameter variation, to seed the phonetic
fingerprint DB with a wide-coverage bank. When the user's
actual voice comes through, it gets compared against a rich
synthetic set rather than an empty DB.

Currently the DB is empty for ~30+ utterances before any
real speaker-match works (every match returns 0.5 prior).
With synthetic seeds, matching could start producing real
similarity scores from utterance #1.

### Why TTS (not just real recordings)

- Paragraphs have variable length but identical text content
  across users — TTS lets us generate language-agnostic
  "what does THIS paragraph sound like" anchors
- Param variations cover the variation space FASTER than
  organic samples — a real user reads a paragraph at one
  pitch/rate/timbre; synthesis can sweep them
- Synth samples can be labeled with their parameters; train
  later models on (audio, text, params)

### Phased delivery (proposed sub-ships)

- [ ] **2.7.1 — TTS infrastructure choice.** Survey:
  - System TTS (espeak-ng, festival, picotts) — free, fast,
    awkward voice
  - Coqui XTTS / Tortoise — high-quality, slow, model file
    on disk
  - Piper TTS (rhasspy) — fast, neural, ~50MB models, good
    quality, runs on CPU
  Likely pick Piper as a feature-gated dependency. Reads
  text → outputs 22050Hz mono WAV in <100ms for typical
  paragraph. Add `--features tts` to Cargo and a
  `~/.config/web2qml/tts_voices/` dir.

- [ ] **2.7.2 — Synth fingerprint generation.** For each
  manpage paragraph, generate N synthetic readings with
  parameter variation:
  - Pitch shift ±20% (3 steps)
  - Speed shift ±30% (3 steps)
  - Mild EQ band emphasis (low/mid/high)
  - Lightweight distortion (saturation, soft-clip)
  - Various noise blends (white, pink, brown @ -20dB SNR)
  - Gaps and stutters (simulate breath, hesitation)
  - Crossfades between paragraphs (simulate flow)
  Run each through the same phonetic.rs fingerprint extractor
  the bg-listen path uses. Store as `synthetic_fingerprints.bin`
  alongside the regular DB, but tagged as synthetic so they
  can be downweighted in matching.

- [ ] **2.7.3 — Synth-aware similarity scoring.** Update
  speaker matching to consider synthetic fingerprints with
  a reduced weight (e.g. 0.3x of organic). When user voice
  comes in, similarity is computed against BOTH banks; a
  match against organic samples wins ties. Goal: synthetic
  fingerprints help cold-start matching but don't dominate
  once real samples accumulate.

- [ ] **2.7.4 — Paragraph-text→synth-fingerprint pre-bake.**
  At first launch (or when manpages registry changes), run
  the synth generation in a background thread. Caches under
  `~/.config/web2qml/synth_cache/<paragraph-hash>/`. Cache
  invalidates when paragraph text changes.

- [ ] **2.7.5 — Small NN model on accumulated samples.**
  Once we have (organic + synthetic) fingerprint pairs with
  labels, train a small classifier (NN or lightweight
  metric-learning) over the bank. Use it as a secondary
  signal that voting overlaps with the existing phonetic
  similarity to flag/resolve disagreement cases. This is
  far-future — file as a target, not a v1.

### Dependencies

- Depends on (or naturally follows): VAD fix landed v0.1.225,
  persistence fix landed v0.1.224.
- Not blocking but synergistic with: reading-overlay redesign
  (v0.1.223 feedback item 4) — the 5-word buffer + syllable
  matching could lean on synthetic-fingerprint-anchored
  alignment.
- **Cross-arc reference (v0.1.241):** the Phoneme alignment
  + tiered reading model arc (next section below) is a
  COMPLEMENTARY arc, not a replacement. The phoneme arc
  produces phoneme-segmented samples (its sub-ship PB3) that
  THIS arc's later phases (2.7.4-2.7.5) would consume as
  training data. Both arcs share the underlying speaker-
  fingerprint storage and corpus directory structure. If
  sequencing is forced, the phoneme arc's PA1-3 should land
  before this arc's 2.7.5 neural-model phase, but the early
  TTS phases (2.7.1-2.7.3) are independent and can ship
  whenever appropriate.

### Honest scope check

This is a multi-month arc. The user's framing ("we can build
out dumb prints and train a small neural model") signals
they understand it's not a one-ship deliverable. v1 of this
arc is sub-ship 2.7.1 alone — pick a TTS engine, get espeak
running as a placeholder, prove the call path. Then 2.7.2's
synth-fingerprint generation. The neural-model piece may
remain aspirational for a long time.

## Phoneme alignment + tiered reading model arc (v0.1.241 conversation)

Designed over an extended conversation in May 2026 about
reading-overlay accuracy, code-pacing, onboarding, and
singing-as-corpus-capture. The conversation walked through
several candidate approaches and converged on a layered
architecture where:

- A cheap "tier 1" model handles real-time word advance
  on any content (silence detection + per-content-type
  multipliers + decay logic)
- An expensive "tier 2" model (phoneme classifier + forced
  alignment) runs occasionally to TUNE tier 1's parameters
  from observation
- Tuned tier-1 parameters persist per-user-per-content-type
- Battery-aware mode selection turns tier 2 off when not
  plugged in or when battery is low
- Tier 2's machinery also powers other features: onboarding
  letter-tracking, terminal voice commands, corpus phoneme
  segmentation, eventually karaoke and pronunciation drills

User framing verbatim across the conversation:

> "for code blocks as reading we need tree-sitter-rs to help
> us pace words they are going too fast right now"

> "if there are 750ms of near-silence... we change a
> multiplier in increments of .05 for the next ten seconds
> so it's a stacking multiplier that each stack turns off
> after five seconds with a decay of .01 per 500ms so it
> takes 5s + 2.5s to ramp down"

> "we have both the name the first time typed out so we
> should try and show them how accurately we can track each
> letter of their voice entered as singing in realtime with
> some sub syllable trickery"

> "song lyrics could be transcribed via whisper and played
> back with voices from the person's library to help with
> their encoding"

> "package some instrumental chords and basic drum patterns
> and show a random word dictionary and force the user to
> rhyme that word at the end when the beat stops"

> "we can fallback to the lazy version as a model that has
> an initial state and is tuned once the better model is
> online and can retime it... then when we want to save
> battery we fall back to the tuned model and check in every
> so often to keep things tight"

> "should we also be hitting our man pages and terminal
> elements with phoneme analysis and synthesis and a
> preloaded database for the common ones?"

### Core architecture: two-tier alignment

**Tier 1 (lazy, always-on)**:
- Per-content-type WPM multipliers (prose, code, list,
  table — each tunable independently)
- Silence-detection bumps the active multiplier DOWN by
  ~0.05 per detected 750ms silence event during reading
- Each bump decays at ~0.01 per 500ms after a 5-second
  active period (total stack life ~7.5s)
- Multiple stacks compose (additive)
- Multiplier floor 0.5x — past that the user has clearly
  stopped engaging; stop bumping
- Stack resets between paragraphs
- Cost: cheap (small state, no neural inference)

**Tier 2 (expensive, on-demand)**:
- Real-time phoneme classifier (small pre-trained model,
  20-50ms inference per frame)
- Forced alignment against expected phoneme sequence
  (from pre-computed database OR runtime G2P)
- Reports "user is actually at phoneme N" vs tier 1's
  "should be at word M"
- Disagreement → adjusts tier 1's multiplier persistently
- Cost: continuous CPU when active (~5-15% of one core)

**Persistence**:
- `~/.config/web2qml/reading_calibration.toml`
- Per-content-type rows with tuned parameters
- Updated by tier 2 when it spot-checks and finds drift
- Loaded at cold-start for tier 1's initial state

**Battery-aware mode selection**:
- Plugged in: tier 2 always-on. Maximum accuracy.
- Battery, recent calibration: tier 1 only, tier 2
  spot-check every 60-120s.
- Battery, stale calibration: tier 2 on for first 30s of
  reading session, then off.
- Low battery (<20%): tier 2 fully disabled. Tier 1 alone
  with persisted model.

**Lifecycle (the user-visible journey)**:

1. **Cold start** (first read-aloud ever): tier 1 with
   reasonable hand-tuned defaults per content type
   (PA1's values). User reads; tier 1 advances words
   approximately. Quality: okay, not great.

2. **Calibration phase** (tier 2 active, first ~30s of
   any session): tier 2's phoneme classifier runs in
   parallel with tier 1. At each tier-1 advance, tier 2
   reports the ACTUAL phoneme position. Disagreement
   feeds back into tier 1's multiplier.

3. **Convergence**: after enough stable-high-confidence
   observations, tier 1's parameters stabilize for this
   user / this content type. Tuned model persisted to
   reading_calibration.toml.

4. **Battery-save mode** (subsequent sessions, on
   battery): tier 2 sleeps. Tier 1 runs alone with the
   tuned parameters. Accuracy is now MUCH better than
   cold-start because parameters were tuned to this user.

5. **Drift correction**: every 60-120s (configurable
   cadence), tier 2 spot-checks for 5-10 seconds. If
   tier 1 and tier 2 agree within tolerance → tier 2
   sleeps again. If drifted (user fatigues, reading
   slows over the session) → tier 2 re-tunes tier 1 and
   sleeps.

**Safety rails on the auto-tuning loop**:

- **Confidence gating**: only update tier 1 when tier 2's
  phoneme-position estimate is high-confidence and
  stable across multiple frames. Single-frame
  classification noise should NEVER reach tier 1's
  parameters.
- **Multiplier floor (0.5x)**: tier 1's multiplier has
  a hard floor. Past that, the user has clearly stopped
  engaging — don't keep stacking decay events.
- **Loop guard**: if tier 1 is already at the floor,
  silence-detection events get logged but don't add
  more stacks. Prevents feedback runaway where slow
  reading triggers slower display triggers slower
  reading.
- **Stack reset between paragraphs**: each paragraph
  starts with the persisted base multiplier for its
  content type, not the live stack from the previous
  paragraph. Inter-paragraph gaps naturally decay any
  remaining stacks.
- **Tier-2 invisibility**: the tuning is silent. No
  "calibrating..." UI. Users shouldn't worry about
  whether the system is calibrated yet. The visible
  behavior is "highlight follows me better this week
  than last week."

**Visible feedback (small)**:

- When tier 1's effective multiplier is not 1.0, show
  a small indicator near the rate button (e.g., "0.85×"
  pill). Communicates that the pace HAS adjusted, why
  the display might not feel like the baseline WPM
  exactly.
- Long-press the indicator reveals: which content type,
  current multiplier, last calibrated <timestamp>.

**Bonus: PC2 onboarding seeds the calibration**:

The onboarding flow (PC2) records the user's voice
under known conditions: typed name, prompt timing,
captured audio. If tier 2 is running during onboarding,
the alignment data observed there can SEED the tier-1
model's initial state BETTER than hand-tuned defaults.
Cold-start for the user's FIRST regular read-aloud
session begins with a seeded model, not pure defaults.

The seeded values are still per-content-type — onboarding
gives us "prose" calibration (the user reading typed
prompts). Code/list/table get refined during subsequent
real reads.

### Phased delivery

#### Foundational phases

- [x] ~~**PA1 — Hand-tuned tier 1 with per-content-type
  multipliers.**~~ **Landed v0.1.242.** No tier 2 yet, no
  auto-tuning. Just baseline multipliers per content type
  (prose=0.95, code=0.7, list=0.9, table=0.85) hand-picked
  from observation. Reading rate adapts to content type out
  of the box. Closes the "code blocks too fast" feedback
  immediately. Smallest possible ship — mostly config
  values + a per-content-type lookup at reading start.
  ~half a ship.
  
  **Concrete implementation plan (PA1 specifics):**
  
  The selection-wrapper already tags paragraphs by
  content type (paragraph, list, table, heading)
  through v0.1.236/v0.1.240 expansion. Code blocks
  arrive wrapped in `<pre>` from qml_gen — selectable
  but currently sharing the prose path.
  
  Files to touch:
  
  - `src/qml_gen.rs`: when emitting paragraph wrappers,
    add a `_contentType` string property to the wrapper:
    - "prose" (default for `<p>`, headings)
    - "code" (for `<pre>` blocks)
    - "list" (for `<ul>`/`<ol>` items)
    - "table" (for table cells/rows)
    
  - `src/manpages.rs`: same content-type tagging for
    the manpage-rendered wrappers (which use
    `qml_gen::maybe_wrap_extended` per v0.1.241
    cleanup pass).
    
  - `qml/main.qml`: readingOverlay gets a new function
    `_baseMultiplierFor(contentType)`:
    ```
    function _baseMultiplierFor(t) {
        if (t === "code") return 0.70
        if (t === "list") return 0.90
        if (t === "table") return 0.85
        return 0.95  // prose default
    }
    ```
    
  - When transitioning state to "reading", read the
    selected paragraph's `_contentType` and set
    `effectiveWpm = wpm * _baseMultiplierFor(t)`.
    The Timer interval is computed from effectiveWpm
    instead of wpm.
    
  - Visible indicator: when multiplier is not 1.0,
    show a small "0.95×" pill next to the rate
    button. Keeps the user informed why pace differs
    from the slider value.
  
  **Multipliers chosen by reasoning, refined later by
  PA2-7's auto-tuning:**
  
  - **prose=0.95**: slight slowdown from the v0.1.241
    default 160 wpm. Addresses the "still a little too
    fast" user feedback without a separate slider
    change.
  - **code=0.70**: code reads ~30% slower than prose.
    Punctuation pauses, identifier-syllable rhythm,
    indentation breath. The v0.1.241 user complaint
    was specifically about code; 0.70 is a starting
    point.
  - **list=0.90**: lists have natural inter-item
    pause; multiplier accounts for the slight slowdown
    of bullet-by-bullet rhythm.
  - **table=0.85**: tables read with cell-by-cell
    rhythm; ~15% slower captures the per-cell pause.
  
  These are PRIORS, not measurements. PA2-7 refine
  them per-user automatically. The point of PA1 is to
  ship a sensible starting point with zero new
  infrastructure.
  
  **Validation criteria for PA1 success:**
  
  - User reads a code block from welcome manpage: pace
    feels right (no more "going too fast" complaint)
  - User reads a prose paragraph: pace is slightly
    slower than v0.1.241 default but not awkwardly so
  - User reads a list: pace feels natural for bullet-
    by-bullet
  - User reads a table: pace feels natural for cell-
    by-cell
  - Indicator visible and accurate when multiplier ≠ 1.0
  
  **Does NOT include in PA1:**
  - Silence detection (PA2)
  - Persistence (PA2)
  - Per-user adjustment (PA2-6)
  - Phoneme classifier (PA4)
  - Battery-aware mode (PA7)
  
  PA1 is the smallest possible useful ship — config
  values + lookup. ~half a session of work.

- [ ] **PA2 — Silence-detection multiplier with decay.**
  Wire the silence-detection signal from voice runtime
  (VAD from v0.1.225). Add stack/decay logic in QML.
  Visual: small multiplier indicator near rate button
  showing current value when not 1.0 (e.g., "0.85×").
  Persistence: last-known per-content-type multiplier
  saved to `reading_calibration.toml` and loaded at
  cold-start. **S-M**.

  **PA2 sub-status (v0.1.368 + v0.1.369 + v0.1.370 + v0.1.371 — infrastructure complete):**
  - ✅ **Plumbing prerequisite landed v0.1.368.**
    `voice::MIN_SILENCE_MS` AtomicU32 + Backend qt_property +
    bucketed signal in pty_read_screen.
  - ✅ **Reading-overlay QML wiring landed v0.1.369.**
    `silenceMultiplier` property + Connections listener +
    stacked effectiveWpm.
  - ✅ **Visual indicator pill landed v0.1.370.**
    `_readSilencePill` Rectangle shows "voice X.XX×" when
    multiplier ≠ 1.0.
  - 🟡 **Persistence infrastructure landed v0.1.371.**
    Backend `reading_calibration_get(content_type) -> f32` and
    `reading_calibration_set(content_type, multiplier)`
    qt_methods. File: `~/.config/web2qml/reading_calibration.toml`
    in `[multipliers]` section format. QML reads persisted value
    at reading-state-enter, falls back to PA1 baseline if none.
    Tiny hand-written TOML parser (no serde_toml dep).
  - ✅ **Auto-blend of in-session observations into persisted
    value** — landed v0.1.381. New
    `reading_calibration_observe(content_type, observed)`
    qt_method blends a single observation into the stored
    multiplier via EMA (alpha=0.10). When no prior value
    exists, the observation becomes the stored value
    directly. Defense-in-depth: rejects non-finite values,
    clamps observed to 0.30..=1.20. Persists via the
    existing `reading_calibration_set` path so file format
    + atomic-ish write are reused. QML reading overlay
    calls this on paragraph advance — backend just blends.
    Resolution: per-paragraph EMA chosen over per-session
    commit because (1) the user can see the calibration
    converging during a single session rather than waiting
    for a session boundary, and (2) the slow alpha (0.10)
    means one weird observation can't move the stored
    value much.

  Audit finding from v0.1.247 still relevant for the
  remaining QML wiring:
  
  **v0.1.247 audit finding:** VAD silence is computed in
  voice.rs's audio loop but not exposed to QML. Concrete
  finding: voice.rs already tracks `silence_ms` per
  speaker (around line 861-883). Each chunk increments it
  by `chunk_duration_ms`. The data PA2 needs already
  exists; it just needs to be DISPATCHED.
  
  Real PA2 sub-tasks:
  1. Add a new qt_signal in main.rs, e.g.
     `voice_silence_tick(silent_ms: u32)` — fires every
     ~250ms with the LEAST silent_ms across active
     speakers (or 0 if anyone's talking). Most relevant
     for reading is "is the user talking" so single
     least-silent value is the right signal.
  2. In voice.rs's audio loop (around line 861), after
     the silence_ms updates, dispatch the signal via
     an mpsc channel from audio thread to main thread.
     Existing voice.rs already does this for other
     signals (search for "tx.send" patterns).
  3. ReadingOverlay listens for the signal, applies the
     decay/stack logic.
  
  Step 2 is the actual work. ~50-100 lines of channel
  plumbing + dispatch. Scope: **S-M** (1-2 focused hours).
  Hardware-validated only after the dispatch is in place
  and you can verify the signal fires correctly during
  actual reading.

- [x] ~~**PA3 — Build-time phoneme database for bundled
  content.**~~ **Landed v0.1.246.** Three pieces:
  - `scripts/gen-phoneme-db.py`: walks bundled manpages,
    splits paragraphs the same way the runtime does
    (h1-h3 / prose / lists / fences, filtered by
    extended_word_bounds), runs each word through
    espeak-ng to get IPA, writes a line-based text file.
    espeak-ng is build-time-only — committed data file
    means end users don't need it installed.
  - `data/phoneme-db.txt`: 15.5 KB, 27 paragraphs, 673
    words across welcome/manpages/onboarding. Line-based
    format chosen over JSON because the project avoids
    serde_json and the existing hand-rolled JSON helpers
    are flat-only. 78% smaller than equivalent JSON.
  - `src/phoneme_db.rs`: include_str!() the data file,
    lazy parse via OnceLock, expose lookup_by_text and
    lookup_any. 6 unit tests. Debug hookups in main.rs:
    startup probe log + per-paragraph-selection
    "phonemes: N words from <manpage>" / "not bundled"
    suffix on the existing [paragraph] log.

- [ ] **PA4 — Runtime phoneme classifier.** Ship a small
  pre-trained phoneme classifier (~5-20MB). Inference
  every 20ms during reading-state. Outputs phoneme
  probabilities. Tier 2 of the architecture above.
  Big lift — adds dependency, model, inference
  infrastructure. **L**.
  
  **v0.1.247 audit finding — sub-prerequisite:** the
  voice runtime currently has NO exposed audio-frame
  surface for non-ASR consumers. ASR runs inside voice.rs
  on the captured stream; nothing else sees the PCM
  buffer. PA4's classifier needs either:
  - Access to the PCM buffer directly (Rust-only,
    cheaper, but tightly couples PA4 to voice.rs internals)
  - A new mpsc channel that fans out audio frames to
    multiple consumers (ASR + classifier; cleaner
    separation but adds latency budget concerns)
  
  Whichever path, this fan-out infrastructure is a
  prerequisite that must land BEFORE PA4 model
  integration. Probably 200-500 lines of voice.rs
  refactor. Should be done in its own ship to avoid
  bundling it with model selection.

- [ ] **PA5 — Online forced-alignment algorithm.** Takes
  tier-2 phoneme outputs + expected phoneme sequence,
  produces "user is at position N" estimate. Viterbi-
  style online variant. Tells tier 1 when its multiplier
  is wrong. **M-L**.

- [ ] **PA6 — Tier-2-tunes-tier-1 wiring.** When tier 2
  is running and disagrees with tier 1 about position,
  update tier 1's multiplier toward what tier 2
  observed. Confidence-gated (only update on stable
  high-confidence stretches, not single-frame noise).
  **M**.

- [ ] **PA7 — Battery-aware mode selection.** Read
  power-supply D-Bus signal (Linux UPower) for plugged/
  battery status and percentage. Switch alignment mode
  based on state. Spot-check timer for periodic tier-2
  invocation. **M**.

#### User-facing features powered by the foundation

- [ ] **PB1 — Read-aloud accuracy upgrade.** Once PA4-6
  exist, replace the WPM Timer for selected paragraphs
  with phoneme-aligned advance. Word highlight moves
  WHEN THE USER ACTUALLY SAYS THE WORD, not on a fixed
  schedule. The teleprompter scroll machinery from
  v0.1.241 still drives the visual; only the advance
  trigger changes. **M**.

- [ ] **PB2 — Terminal voice command recognition.**
  Build-time phoneme database for common Unix commands
  (ls, cd, git status, etc.). When user speaks in
  terminal context, match audio against the indexed
  command phonemes. Cheaper and more reliable than
  general whisper transcription for short commands.
  **M**.

- [ ] **PB3 — Corpus phoneme segmentation.** Every
  corpus recording stores phoneme timestamps as
  metadata. Future TTS training pipeline consumes
  phoneme-segmented data. **S** (mostly metadata
  plumbing once PA4-5 exist).

- [ ] **PB4 — User-grown phoneme database.** Every text
  the user engages with gets G2P'd at runtime (if not
  already in the build-time database) and cached.
  Database accumulates per-user over time. **S-M**.

#### Singing capture + onboarding arc

- [ ] **PC1 — Sing-challenge corpus mode (basic).** New
  virtual page `web2qml://sing`. UI:
  
  - "tap to start" button
  - Plays 4 bars of instrumental loop
  - Shows random word from rhyme dictionary on screen
  - On beat 7, music fades to silence
  - User must say a word that rhymes with the target
    into the silent gap
  - Music returns for 2 beats
  - Recording stops automatically
  
  Stored as `corpus/<speaker_id>/challenge/<unix_ms>.{opus,txt,meta.json}`.
  Meta carries target word, transcribed actual word,
  beat schedule, backing track filename.
  
  Whisper transcribes the gap audio. Compare actual vs
  target rhyme phonemes (need PA3 G2P or runtime
  fallback). Mismatch ≠ failure — humor framing, still
  saves the recording.
  
  Initial assets: ship a small CC0 instrumental library
  (5-10 loops, various genres). Optional Music folder
  permission for user-provided backing tracks (deferred).
  
  Rhyme dictionary: small curated list of ~200 words
  with rich rhyme classes. Build-time generated from
  CMU pronouncing dict; ship as JSON.
  
  **M+** (corpus mode + assets + UI). Standalone — does
  NOT depend on tier-2 phoneme infrastructure. Whisper
  alone handles transcription.
  
  **Why this is corpus gold (justification preserved
  from conversation):**
  
  - Speech operates in a 5-7 semitone pitch range;
    singing pushes to 12-18 semitones. TTS models built
    from speech corpus alone can never synthesize the
    user's voice at pitches the corpus never captured.
    Sing samples close this gap.
  - Speech rarely sustains vowels >200ms; singing
    routinely sustains 1-3 seconds. Sustained-vowel
    formants are much cleaner to model than vowel-in-
    flight during fast speech.
  - Consonant-vowel transitions in singing happen at
    varied speeds that reveal articulator habits the
    user's speech doesn't expose at scale.
  - **The "remix" property** — when people improvise
    singing, they unconsciously reproduce phrasing,
    melodic contours, and vocal mannerisms of singers
    they've internalized. A user's brief sung rhyme
    carries their fingerprint of taste. Multiple
    backing tracks (each from different genres in
    their music library or our CC0 library) elicit
    DIFFERENT prosodic registers from the same
    speaker. Hip-hop instrumental → rap cadence.
    Folk fingerpicking → softer/slower. Synthwave →
    belt. One speaker, many registers — exactly what
    TTS models need for style coverage.
  - Constrained-improv games are replay-value-high.
    Users do it more than once per session because
    each round is different. Sustained engagement
    over weeks/months produces startling amounts of
    varied corpus data.

- ✅ **PC2a — Spoken-name introduction (the scaled-down half
  of PC2).** Landed v0.1.359. The existing onboarding flow
  already prompts the user to read a pangram aloud (captures
  the fingerprint) and then asks for their typed name. PC2a
  adds a follow-up step AFTER the typed name: an optional
  "say your name" capture stored to
  `~/.config/web2qml/corpus/<speaker_id>/onboarding/spoken_name.wav`
  with a meta.json carrying the typed spelling. Pronunciation
  preserved separately from spelling.

  Implementation:
  - New `corpus_save_spoken_name(speaker_id, typed_name)`
    qt_method on Backend.
  - Two public helpers added to `corpus_recorder.rs`:
    `current_sample_rate()` and `write_wav_public()` so the
    spoken-name path doesn't reimplement WAV encoding.
  - `speakerRegistrationPopup` extended with a multi-step
    state machine (step 0 = name+color, step 1 = spoken-name
    capture). Step 1 is fully skippable.
  - Mic button with three visual states: idle (accent), recording
    (red, pulsing), saved (green check). Errors fall back to
    idle with retry.
  - Auto-close timer after successful save so the user sees
    the ✓ and the popup dismisses on its own.
  - Skip / Cancel / Close button text adapts to current state.

  Why split from full PC2: the SINGING part needs music
  backing tracks (CC0 instrumental loops) + beat-aware
  playback infrastructure that doesn't exist yet (PC1).
  Spoken-name standalone uses only what's already there
  (voice_listen_dual_async, last_audio_buffer, corpus
  storage). High day-one value: pronunciation captured,
  voice attribution feature discovered during onboarding,
  labeled "user saying own name" audio in corpus for future
  TTS work.

- [ ] **PC2b — Sung-name layer on top of PC2a.** First-
  launch flow uses sing-challenge infrastructure for a
  guided enrollment moment beyond just spoken:
  
  1. Text intro asks user for their name (typed)
  2. Brief spoken-intro capture: "say your name"
  3. Surprise music fade-in
  4. Prompt fades to: "now sing 'hello there, my name
     is [their name]'" in user's locale language
  5. Recording captures while user sings (or refuses
     via skip button — refusal is fine, captures spoken
     only)
  6. End-of-onboarding: tuned tier 1 model is seeded
     from this session's data (if tier-2 is running);
     otherwise spoken sample is at least in corpus
  
  Stored in `corpus/<speaker_id>/onboarding/` with
  language tag and the user's typed-name spelling.
  
  Refusal handling: skip button always visible. "I
  don't want to sing" continues with spoken sample
  only. Multiple skip-paths through.
  
  Cross-cultural: offer hum/rhythmic-spoken alternative
  to the sing prompt — same prosodic-range value
  without the implicit Western-pop performance
  expectation.
  
  **M+** (depends on PC1 infrastructure).
  
  **Data captured by end of onboarding:**
  
  - Typed name (spelling, exactly as user writes it)
  - Spoken name audio (pronunciation, not just spelling
    — enables future feature: app pronouncing the user's
    name back correctly)
  - Sung "hello there, my name is X" audio (or rhythmic-
    spoken alternative if user picked the cross-cultural
    option, or skipped — in which case only spoken
    sample exists)
  - Inferred language (whisper transcribes the spoken
    intro; language detection on the result; ground-
    truth language signal that voice_lang.rs would
    otherwise infer ad-hoc)
  - Voice fingerprint baseline (multiple registers if
    sung version exists — speaking baseline + sung
    amused — gives future affect detection two
    emotional anchors to calibrate from on day one)
  - WPM baseline (how fast user spoke vs the prompt's
    expected timing)
  - Pitch range baseline (from the sung version, if
    captured — much wider than speech alone provides)
  
  **Consent gate before music plays:**
  
  Recording someone singing their own name into their
  device is a different intimacy than typing a name.
  Before the music fade-in, show a brief framing:
  
  > "We're about to record some audio. It stays on
  > your device, never uploaded, and helps the app
  > understand you better. Okay?"
  
  Music demo plays WITHOUT recording so user can
  preview the vibe before committing. "Okay" gates
  the actual capture. Skip path always available.
  
  **First-launch music asset constraint:**
  
  At first launch, user hasn't granted music-folder
  permission yet (and shouldn't have to — that's a
  later prompt). Onboarding uses the CC0 fallback
  library shipped with the app. Music-folder
  integration (for richer challenge variety) comes
  AFTER onboarding completes, as a separate prompt
  the user can decline.
  
  **Why the "your language" prompt is harder than it
  sounds:**
  
  To KNOW which language to use for the prompt before
  the user speaks, we'd need to fall back to system
  locale. Whisper detects actual language AFTER the
  spoken sample. If detected language differs from
  system locale, optionally re-prompt in the detected
  language (or just continue — locale-mismatch is
  itself useful signal that the user prefers a non-
  default language).
  
  First-prompt-in-system-locale is achievable.
  First-prompt-in-actual-language is stretch.

- [ ] **PC3 — Real-time letter tracking during sing-
  onboarding.** During the sung-name section, show the
  typed name in big letters. As the user sings, letters
  highlight in real time corresponding to phoneme
  position.
  
  Cheap version: energy-based syllable detection;
  letters light up in syllable groups. Works without
  PA4 phoneme classifier.
  
  Full version: phoneme classifier + forced alignment
  against the name's phoneme sequence. Sub-syllable
  precision; individual letters light up in order.
  Requires PA4 + PA5.
  
  Phase as two sub-ships: PC3a (syllable groups,
  cheap), PC3b (sub-syllable letters, full).
  
  Why this works visually even when imperfect: ground
  truth is the user's own typed name, so the highlight
  always lands on SOME letter of their name. Worst-
  case error is "highlight is slightly off"; never
  "highlight is on a random Q." User reads the slight
  lag as "system is tracking me, just a tiny bit
  behind."
  
  **PC3a algorithm sketch (cheap, no phoneme model):**
  
  1. Pre-compute syllable boundaries of the typed name
     (CMU dict for English; espeak phoneme output for
     other supported languages; fall back to vowel-
     letter heuristic for unsupported scripts)
  2. Real-time: extract per-frame energy from mic audio
     (already available in voice runtime)
  3. Detect energy bursts above a moving baseline →
     each burst is one syllable
  4. On Nth burst, highlight the Nth syllable's letter
     group in the visual
  5. Latency: ~20-40ms (one frame delay). Visible to
     eye but matches the user's actual onset timing
  
  No phoneme classifier needed. Works at first launch
  on day one.
  
  **PC3b algorithm sketch (full, phoneme-classifier-
  driven):**
  
  1. Pre-compute phoneme sequence of the typed name
     (PA3 G2P)
  2. Pre-compute letter-to-phoneme mapping with
     "phoneme groups" — clusters of letters that map
     to a single phoneme. Examples:
     - "Knight" → letters K-N-I-G-H-T → phonemes
       /n/ /aɪ/ /t/ → letter groups: [K,N], [I,G,H],
       [T] (silent K, silent G+H)
     - "Alexandra" → A-L-E-X-A-N-D-R-A → phonemes
       /æ/ /l/ /ɪ/ /g/ /z/ /æ/ /n/ /d/ /r/ /ə/ —
       relatively clean 1-1 with letter groups
       [A], [L], [E], [X], [A], [N], [D], [R], [A]
  3. Real-time: phoneme classifier outputs per-frame
     phoneme probabilities (PA4)
  4. Online forced alignment (Viterbi-style, PA5)
     tracks current position in the expected sequence
  5. Highlight the letter-group corresponding to
     current phoneme position
  6. Silent letters (K in Knight, G+H in night) get
     highlighted alongside the preceding/following
     real phoneme — they share a group with the
     non-silent letters that "carry" them
  
  **Edge cases captured from conversation:**
  
  - **Non-Latin scripts** (Cyrillic, CJK, Arabic,
    Devanagari): CMU dict is English-only. espeak
    covers many languages with reasonable quality.
    For unsupported scripts, fall back to PC3a
    (energy-based syllable groups, no phoneme
    matching). Still impressive, just coarser. Don't
    block first-launch enrollment on script support.
  
  - **Sung formants ≠ spoken formants**: when singing,
    vowels stretch and formants shift slightly. Most
    formant trackers handle this. For off-key belted
    names, accuracy degrades — fallback path is
    PC3a's energy bursts.
  
  - **Mispronunciation correction loop**: the typed
    name's CMU-dict pronunciation might not match how
    the user actually pronounces their name (foreign
    names, unusual spellings, family pronunciations).
    After the sing capture, optionally show:
    
    > "Did we get this right? [yes / correct it]"
    
    "Correct it" lets the user type the phonetic form
    or re-record. The correction becomes corpus data
    about how this person specifically pronounces
    their own name — valuable for the future TTS
    feature that says their name back.
  
  - **Cold-start: no user-trained model**: the phoneme
    classifier must work on day one before any user
    audio has been captured. Generic pre-trained
    model (wav2vec2-style, ~5-20MB). Not user-tuned
    on day one. Over time, the user's actual phoneme
    patterns refine the classifier (PA arc later
    phases).
  
  **M** (PC3a) + **M-L** (PC3b).

- [ ] **PC4 — Karaoke mode.** Once PA4-5 + PC3 exist,
  a karaoke variant: user pastes/selects lyrics
  (their own, public-domain, or LLM-generated), mic
  captures while highlight scrolls. Pre-aligned
  audio+text pairs without needing post-hoc whisper.
  Reuses tier-2 forced alignment infrastructure.
  **M** (depends on PA4-5).

- [ ] **PC5 — Sing-the-paragraph variant.** Reading
  overlay variant where the user sings the paragraph
  in some melody. Same corpus benefit as PC1 but
  generates phoneme-segmented sung samples of arbitrary
  prose text. Maximally useful for vowel/pitch sample
  diversity. **S** addition once PC1 exists.

### Sequencing rationale

User-perceived order builds anticipation:
1. PA1 ships → "code blocks read at the right pace now"
2. PA2 ships → "the system slows down when I'm falling
   behind"
3. PA3 ships (invisible to user) → phoneme database
   exists
4. PA4-5 ship → tier-2 alignment exists
5. PB1 ships → "reading highlight follows me, not a
   timer"
6. PC1 ships → "we can capture vocals over instrumentals
   for the corpus"
7. PC2-3 ship → onboarding feels magical: "the app shows
   my name letters lighting up as I sing"
8. PC4-5 ship → karaoke + paragraph-singing modes

Each ship delivers visible user value. The biggest "wow"
moment (onboarding letter tracking) sits late so the
infrastructure underneath is solid by then.

### Dependencies summary

- PA1 standalone (no deps)
- PA2 depends on existing VAD signal (v0.1.225)
- PA3 depends on build-time G2P tool choice
- PA4 depends on phoneme classifier model selection
- PA5 depends on PA4
- PA6 depends on PA1 + PA5
- PA7 depends on PA6, plus power-state platform integration

- PB1 depends on PA4 + PA5
- PB2 depends on PA3 (commands DB) + PA4 (matching)
- PB3 depends on PA4 + PA5
- PB4 depends on PA3 (runtime G2P fallback)

- PC1 standalone (whisper-based, no tier 2 needed)
- PC2a standalone (uses existing voice capture + corpus
  storage; SHIPPED v0.1.359)
- PC2b depends on PC1 (singing layer needs music backing)
- PC3a standalone (energy detection only)
- PC3b depends on PA4 + PA5
- PC4 depends on PA4 + PA5
- PC5 depends on PC1

### Relationship to existing arcs

This arc COMPLEMENTS the TTS-bootstrap arc above:
- TTS-bootstrap arc produces synthetic fingerprints from
  text for cold-start speaker matching
- This arc produces phoneme-segmented samples (PB3) that
  the TTS arc's later phases (2.7.4-2.7.5) consume for
  training data
- Both arcs share the underlying speaker-fingerprint
  storage and corpus directory structure
- Sing-challenge mode (PC1) generates corpus samples
  that BOTH arcs benefit from

### What this arc replaces / supersedes

- Adaptive-WPM-from-silence as a standalone feature
  (the conversation walked it back twice; PA2 captures
  the same idea inside a more complete architecture)
- Tree-sitter-rs pacing for code (PA1's per-content-type
  multiplier handles code via a fixed lower value at
  ~0.7x; tree-sitter would only help if we wanted
  per-token pacing WITHIN code, which is a follow-up
  refinement of PA1 once we have data on whether the
  uniform "code = 0.7x" is enough)
- Standalone "sing onboarding" idea (PC2 is the
  implementation; depends on PC1 infrastructure)
- Standalone "letter tracking" trick (PC3 is the
  implementation; phased into cheap+full versions)

### Honest scope check

This is a 4-6 month arc to complete. Not a single ship.
The user understands this and the sequencing above
spreads value across the arc so each individual ship
delivers something visible.

The CHEAPEST useful ship is PA1 (per-content-type
multipliers, hand-tuned). It addresses one of the
v0.1.241 hardware-test complaints ("code blocks are
going too fast") with maybe half a ship of work.

The MOST EXPENSIVE single ship is PA4 (phoneme
classifier integration). Adds a model file, dependency,
inference path. Significant work but unlocks everything
in PB and PC3b/PC4 downstream.

Recommended order: PA1 → PA2 → PC1 (lets us start
capturing varied corpus while the heavier infra
develops) → PA3 → PA4 → PA5 → PB1 → PA6 → PA7 →
PC2 → PC3 → PB2 → PB3 → PB4 → PC4 → PC5.

That ordering puts visible user value early and
defers the heaviest-cost item (PA4) until after PC1
has shown that varied corpus capture works at all.

## Terminal performance investigation (v0.1.241 conversation)

User report: "performance is also slow now in the terminal so
how can we profile this?"

The honest first step is data — we don't know what's slow. The
terminal has accumulated significant visual work across recent
ships (transparency Qt.rgba on every render, expanded selection
wraps, reading overlay teleprompter scroll, theme picker
expansion, etc.) and the regression could be from any of them
OR from compositor-level issues we've already flagged.

### Profiling tooling already available

- `WEB2QML_PROFILE=1` env var enables profiling.log writes
  every 3s (v0.1.170 infrastructure). Current QML accumulator
  may need extending with terminal-specific counters.
- `profiling.log` is `~/.config/web2qml/profiling.log` —
  one JSON line per 3s window.

### Investigation phases (file-don't-fix order)

- [x] ~~**TP1 — Audit current profiling.log coverage.**~~
  **Landed v0.1.244.** Existing infrastructure (v0.1.170)
  captures frame deltas (5-number summaries), touch counts,
  labeled events, page URL. Documented in
  `docs/troubleshooting.md`. Added gaps: tab-change events
  (`tab:term`/`tab:browse`) and alt-screen state transitions
  (`altscreen:enter`/`exit`). PTY bytes/sec and cell-update
  counts not yet added — defer until TP4 data shows whether
  they're needed.

- [ ] **TP2 — Add light timing instrumentation to suspected
  hot paths.** Gated on WEB2QML_PROFILE=1 so zero cost in
  normal use. Cover:
  - `pty_term.rs` cell update loop
  - `qml_gen.rs` HTML rendering (per-document time)
  - Voice runtime (existing log_summary may cover; verify)
  - Reading overlay binding re-evaluations
  Use `eprintln!("[TIMING] X: {:?}", elapsed)` or
  structured JSON to the profiling.log. **S-M**.
  
  **Partial v0.1.244**: tab-change and alt-screen event tags
  added. Per-path timing instrumentation deferred until TP4
  data shows where the time actually goes — premature
  instrumentation in paths that turn out to be cheap is
  noise.

- [x] ~~**TP3 — Document the perf-recording protocol.**~~
  **Landed v0.1.244.** New `docs/troubleshooting.md` with:
  enabling instructions, log format reference, all labeled
  events captured, Konsole-baseline comparison protocol
  (shell + TUI workloads), quick `jq` analysis snippets.

- [ ] **TP4 — User runs the protocol; we analyze together.**
  Hardware investigation, not code work. The user produces
  the log; we look at the data; the actual perf-fix ships
  are downstream and depend on what the data shows. No
  code in this sub-ship — it's the "wait for ground truth"
  step.
  
  **Konsole-baseline comparison (added v0.1.241 conversation):**
  User explicitly named Konsole as the speed reference. Run
  the same workload in both web2qml and Konsole to quantify
  the gap:
  
  - Workload candidates organized by category:
  
    **Shell workflows** (continuous output, scrolling):
    - `cat /var/log/journal-export.log` (or any large
      log; 500K+ lines)
    - `find /usr/share | head -5000`
    - `seq 1 50000`
    - Scroll through a paged file with `less +F`
    - Type into a shell continuously for 30s
    
    **TUI workflows (PRIMARY focus per v0.1.241 user
    feedback — most slowdown lives here):**
    - Open `helix` on a 1000-line source file; scroll
      rapidly (`ctrl-d` / `ctrl-u` repeatedly), watch
      syntax highlighting redraw
    - Run `htop` and let it refresh for 30s with sort
      mode toggled a few times
    - Use `gitui` to navigate through a large repo
      (50+ commits, several branches)
    - Run a TUI git log browser through many commits
    - Open `helix` in split-pane mode and switch between
      panes rapidly
    
    The TUI workloads measure the regime where the
    "stable scrollback + small hot region" assumption
    breaks down. If web2qml's terminal is noticeably
    slower than Konsole here but matches Konsole on
    shell workflows, that tells us the slowdown is
    specifically in the "many cells changing in bursts"
    case — which points at the regime-aware refinement
    in Option F below.
  
  - Per-workload metrics in each terminal:
    - Wall time to render
    - Peak CPU% during output (`pidstat -p $(pidof X) 1`)
    - Memory delta (RSS before/after)
    - Subjective: did typing feel laggy, did scrolling feel
      smooth, did the cursor stutter, did the TUI feel
      sluggish vs snappy
  
  Expected outcome categories that drive different
  follow-ups:
  - **<2x slower at <2x CPU on ALL workloads**: gap is
    closeable via localized optimizations. File specific
    perf-fix sub-ships.
  - **<2x slower on shell workflows, but 3x+ slower on
    TUI workflows**: the regime-specific case (user's
    flagged hypothesis). Suggests the per-cell QML
    overhead dominates when cells are changing in bursts.
    Foveated with regime awareness (Option F) is the
    targeted fix.
  - **2-5x slower or 2-5x CPU across the board**:
    significant inefficiency somewhere specific. Profile
    to find it. Probably one or two hot spots account
    for most of it.
  - **>5x slower or sustained high CPU during simple
    operations**: architectural cost of QML scene-graph
    composition for terminal rendering. Triggers
    consideration of options below.
  
  **If architectural cost is the issue, options:**
  
  - **Option C: Custom QQuickItem for terminal pane.** Drop
    from QML-composed cells to a single QQuickItem with
    custom `updatePaintNode` rendering the entire terminal
    grid directly. Konsole-equivalent rendering model.
    Significant rewrite of pty_term.rs's QML integration.
    Maybe a month of focused work. Rest of app stays QML.
    
  - **Option D: Embed alacritty's renderer.** Open library
    that's already solved character-grid rendering. We lose
    some integration with our QML chrome (touch row, voice
    commands routing through the same input pipeline) but
    gain proven-fast rendering. Engineering cost similar to
    Option C; difference is "build vs reuse."
    
  - **Option E: Embed Konsole via kparts.** KDE apps can
    embed Konsole as a kpart. On KDE/Asahi this would
    "just work." Cross-DE fragility. Lose any customization
    of the terminal pane (no transparent overlap with our
    bg_opacity, no integrated touch row). Probably too lossy
    on integration to be acceptable.
  
  - **Option G: 2x2 tile grouping with shared data pointers.**
    User suggestion v0.1.241 (refining away from Option F as
    the first-line architectural response). Reduces scene-
    graph item count without changing the rendering model
    much.
    
    Current model: one QQuickItem per cell. An 80x24
    terminal has 1920 items. Each cell change → property
    cascade → scene-graph update per item.
    
    Tile model: group cells into 2x2 tiles, each tile being
    a single QQuickItem with custom paint (updatePaintNode
    or QQuickPaintedItem). Each tile holds a REFERENCE to
    the canonical cell grid (Rust-owned), not copies. When
    any of the 4 cells changes, the grid sends ONE
    invalidate signal to the tile; the tile re-renders all
    4 cells from the shared data during the next paint.
    
    Item count: 80x24 cells → 40x12 = 480 tiles. 4x
    reduction in QQuickItems.
    
    Change-propagation: 500-cell helix repaint → at most
    500 tile invalidations (typically fewer because TUI
    updates cluster). Replaces 500*4 = 2000 property-setter
    notifications with 500 or fewer invalidates.
    
    Memory: one canonical cell grid in Rust. Tiles hold
    references, not copies. No N-times memory multiplier.
    
    Animations: per-tile granularity, not per-cell. A
    single-character change in the middle of a tile
    re-renders all 4 cells with the animation effect.
    Acceptable degradation; only 4 cells are affected.
    
    Theme overlays: applied at the tile level via QML
    properties (each tile is still a QML item). Tiles
    inside an overlay region get tinted differently. Same
    mechanism as today.
    
    TUI repaints: 1920-cell full-screen repaint →
    480 tile invalidations instead of 1920 item updates.
    Pure rendering cost unchanged (still painting all
    cells); orchestration cost drops 4x. The QML overhead
    that dominates today's TUI case shrinks proportionally.
    
    Why 2x2 specifically:
    - 1x1 (current): max granularity, max overhead
    - 2x2: 4x reduction, still small enough for per-tile
      animation, single-cell change repaints only 4 cells
    - 4x4: 16x reduction, single-cell change repaints 16
      cells (cursor moves redraw 16-cell area)
    - 8x8 / row / screen: large batches but wasteful per-
      cell, lose animation granularity
    
    2x2 is the midpoint where item-count reduction is
    significant but per-update rendering cost stays low.
    Could re-tune to 3x3 or 4x4 based on profiling — the
    architecture is the same.
    
    Implementation cost:
    - Custom QQuickItem (tile widget) with updatePaintNode
      doing direct QPainter rendering of 4 cells
    - Grid->tile invalidation mapping (when cell (r,c)
      changes, mark tile (r/2, c/2) dirty)
    - QML still controls layout (Grid of tiles), gestures,
      theme bindings — only the inside of each tile is
      custom
    - Probably 300-500 lines of Rust + minor QML
    - 2-3 weeks of focused work, vs 2-3 months for Option F
    
    Doesn't preclude Option F: if 2x2 tiling closes most
    of the gap and we still want more, foveated can layer
    on top (TUI mode uses 8x8 tiles, shell mode uses 2x2;
    cell-fade animations only in hot region; etc.). The
    tile data structure is the foundation foveated would
    build on.
    
    Trade-offs vs continuing with 1x1 cell items:
    - + Significant scene-graph cost reduction (~4x)
    - + Memory cleaner (one canonical grid, references)
    - + Per-tile invalidation cheaper than per-cell
      property cascades
    - - Animation granularity reduced to tile level
    - - Custom paint code to write/maintain
    - - Per-tile theming bindings need wiring up
      (one-time cost)
    
    Why this is probably the right FIRST architectural
    move:
    - Cheap to build (weeks not months)
    - Addresses the most likely cost source identified by
      reasoning about TUI updates
    - Doesn't compromise integration (tiles are still QML
      items)
    - Doesn't preclude further investment (foveated can
      layer on)
    - Reversible (could undo if tiling didn't help and we
      had clearer data on what does)
    - Conservative — closest possible architecture to what
      we have today
  
  - **Option F: Foveated rendering with regime awareness —
    Konsole-cheap cold + QML-rich hot.** User suggestion
    v0.1.241: split the terminal pane into a hot region
    (cursor, animations, recent-activity bounding box)
    using full QML composition + theme overlays + per-cell
    animations, and a cold region (everything else) using
    direct QPainter rendering to a cached texture.
    
    The insight: attention is non-uniform across the screen,
    so rendering effort can be too. Most of the terminal at
    any moment is static text the user's eye has moved past;
    only a small region near the cursor and recent activity
    needs rich rendering.
    
    **Regime-aware refinement (v0.1.241 user follow-up):**
    The hot/cold split as initially sketched assumes mostly-
    stable scrollback with a small active region — true for
    shell workflows but NOT for full-screen TUIs. The user
    specifically flagged that TUI apps (helix, gitui, htop,
    vim) are likely where the most slowdown lives, because
    those apps repaint large screen regions on their own
    schedule and the "hot region" concept inverts: most of
    the screen is in flux.
    
    So Option F really has three regimes the rendering layer
    needs to recognize:
    
    1. **Idle / shell prompt**: very little is happening.
       Cursor blinks, maybe a status line updates. Foveated
       with small hot region around cursor is ideal.
    
    2. **Shell output streaming**: text arrives in bursts.
       New rows momentarily hot (per-cell fade animations
       play), bake into cold quickly. Hot pool sees
       moderate churn but stays bounded.
    
    3. **TUI active (alternate screen mode)**: the TUI
       redraws large regions on its own schedule. Hot
       region concept INVERTS: the entire TUI display is
       cold (cheap rendering), and only OUR overlay chrome
       (voice command indicators, hover affordances) is
       hot. Animations on cell updates are SUPPRESSED in
       this regime (the TUI is making its own visual
       decisions; our animations would fight with them).
    
    Regime detection: existing PTY state already tracks
    alt-screen mode (smcup/rmcup escape sequences that
    helix/vim/htop/less etc. emit). When alt-screen flag
    is set, switch to TUI regime. When it clears, revert
    to shell regime. Output rate over a rolling window
    distinguishes idle from active shell.
    
    Mechanics:
    - Hot region = union of bounding boxes around cursor +
      recently-changed cells (decaying over ~300ms in shell
      mode, suppressed in TUI mode) + mouse pointer +
      active TUI focus indicator + our overlay chrome
    - Hot region renders as QML cells from a fixed pool of
      ~100-300 items in shell mode, ~50 items in TUI mode
      (just chrome)
    - Cold region renders as a single QPainter-painted
      texture; updated lazily only when cold cells change
    - Cells transition between cold and hot at their
      bounding boxes; matching glyph rendering between
      pipelines is required to avoid a visible seam
    
    Performance properties:
    - Idle: one textured quad per frame. Cheaper than
      Konsole (which redraws cells each frame).
    - Shell output: hot-region pool bounded, surplus
      falls back to cold-path rendering. Animations get
      clipped during overflow, but text remains correct.
    - Scrolling: cold texture shifts vertically (one
      blit); only newly-visible rows need new rendering.
    - TUI mode: entire grid renders via cold path; hot
      pool only used for our own overlay chrome. Matches
      Konsole/alacritty perf since we're doing
      essentially the same work as them in this mode.
    
    Implementation cost: substantial. ~2-3 months for a
    polished version. Two pipelines must stay visually
    matched (glyph parity), pool manager, region tracker,
    texture updater. More complex than Option C alone
    (~2-3x complexity) but preserves more integration
    capability than C/D/E.
    
    The regime-aware refinement actually SIMPLIFIES the
    TUI case: in TUI mode, only the cold path is active.
    The complexity of "two pipelines must stay matched"
    only applies in shell/idle modes where the hot region
    is rich-rendered. So the architectural investment is
    "build the cold path well"; the hot path is additive
    on top for the shell case.
    
    Risks: glyph parity between QPainter and QML rendering
    is achievable but requires careful enforcement and
    visual regression testing. Animation transitions across
    the cold/hot boundary need to bake their end-state into
    the cold texture cleanly. Mode-switch transitions
    (entering/leaving alt-screen) need to be visually
    seamless or the TUI launch/exit will flash.
    
    Why this is potentially the best option for the
    architectural case: it preserves theme overlays
    (apply to both pipelines in shell mode; apply to cold
    pipeline in TUI mode), animations (live in hot region
    by definition; suppressed in TUI mode anyway),
    voice command UI overlays (extend the hot region to
    wherever the overlay is, in any regime), AND gives
    Konsole-or-better steady-state perf. The integration
    thesis (rich chrome over a fast terminal) is
    fundamentally compatible with this design where it
    fights against C/D/E.
  
  None of C/D/E/F/G are decided in this sub-ship — they're
  documented so the data-gathering phase has clear options
  to drive toward.
  
  **Tentative ordering of preference** (subject to profile data):
  1. **Option G (2x2 tile grouping)** — cheapest first move,
     conservative, addresses the most likely scene-graph
     bottleneck, doesn't preclude F. Try this first if
     architectural change is warranted.
  2. **Option F (foveated regime-aware)** — if G is
     insufficient, layer foveated on top of G's tile
     infrastructure. Tiles in TUI mode get bigger (4x4 or
     8x8), tiles in shell mode stay 2x2 with rich treatment.
  3. **Option C (custom QQuickItem)** — if both G and F are
     somehow insufficient, drop QML entirely for the
     terminal pane. Most expensive integration cost.
  4. **Option D (embed alacritty)** — alternative to C using
     existing library. Similar engineering cost, less
     custom code, more dependency.
  5. **Option E (embed Konsole)** — fallback only if all
     above fail. Loses too much integration.

### Hypotheses worth checking (ordered by likelihood)

1. **Transparency causing overdraw on Wayland/KWin.**
   The translucent QQuickWindow surface format may force
   software compositing or sub-optimal rendering paths on
   this platform. Test: temporarily force `bg_opacity = 1.0`
   in QML and see if perf returns. If yes, transparency is
   the cost. Already filed as M — transparency flickers
   (Wayland/KWin compositor); perf regression may share
   root cause.

2. **Binding loops or excessive re-evaluation.**
   QML bindings on properties that change every frame can
   multiply. The reading overlay's per-word opacity Behaviors
   (1200ms NumberAnimation) animate ~50 words simultaneously
   when currentWordIndex changes. The teleprompter Flow's y
   binding adds another animation. None alone should be a
   killer but combined with terminal rendering they might
   accumulate.

3. **String allocation in hot paths.**
   Recent cleanup pass (v0.1.241 maybe_wrap_extended) took
   `qml: String` by value to avoid extra copy, but other
   hot paths might have grown extra allocations. The cell
   rendering in pty_term.rs is the most suspect for the
   terminal-specific slowness.

   - [x] ~~Dirty-rect arc stage c — per-row packed cache for the native
     renderer.~~ DONE v0.1.892. The native Canvas path (`term_screen_runs`)
     re-packed the WHOLE visible screen every frame — `row_runs_packed(r)`
     for every row, each doing render_row_runs (run coalescing + a Vec + a
     String per run) + a `format!` per run. Now a per-visible-row cache
     (`row_pack_cache` on TermGrid) holds each row's packed string and
     `row_runs_packed_cached` recomputes only on a miss; `term_screen_runs`
     reuses the rest. So a frame that touches K of N rows does O(K) packing
     work instead of O(N). Correct by construction: the cache is invalidated
     in `mark_row_dirty` (one row) and `mark_all_dirty` (all), the exact hooks
     the existing render-gating already trusts — and every content change
     (cell write, scroll/scrollback shift, resize, alt-screen swap, bulk
     erase) already routes through one of them, verified at their call sites.
     Safety net: a unit test asserts the cached output is byte-identical to a
     fresh `row_runs_packed` across miss/hit/invalidate/mark-all paths, so a
     hit can't drift from the truth. This is the localized form of the win
     Option G chases at the QML-item layer; the two are complementary (G still
     reduces scene-graph item count). Doesn't touch the HTML path or the
     snapshot migration the code comments flag as the next native cleanup.

4. **Memory pressure causing swap.**
   If web2qml's resident memory has grown over many ships
   and the user's RAM is pressured, kernel swap-in/out shows
   up as random latency. Check with `top` / `htop` during a
   slow session.

5. **Scraper / qml_gen background work.**
   If terminal feels slow but really the BROWSER tab
   background work is competing for CPU (still parsing a
   recent page load), that's a different problem. The
   browser pane should be idle when the terminal tab is
   active — verify via profiling.

### Fixes (deferred until data exists)

- [ ] **TP5+ — Specific fixes based on findings.**
  Format: each identified hot spot gets its own sub-ship
  with hypothesis + measurement + fix. Don't pre-write
  these; they should fall out of the data.

### Scope check

The PROFILING work is contained (S-M total for TP1-3).
The FIX work depends entirely on what we find. Could be
half a ship if it's a single hot loop; could be M+ if
it's a deeper architectural issue (e.g., we discover
that translucent windows on KWin are fundamentally a
problem we can't fix from app side, in which case the
"fix" becomes "default to opaque + offer opt-in
transparency with a warning").

## Folder picker for network mounts on slow wifi (v0.1.241 conversation)

User report: "folder picker showing ssh and device folders
on low bandwidth cafe wifi... how can we make this work"

Two problems coupled:

1. **Performance**: Qt FileDialog enumerates GVFS / SSHFS /
   SMB / MTP mounts at open time. On slow network, mount
   probes block dialog open for seconds.

2. **Privacy**: SSH server addresses, NAS shares, mobile
   phone names appear in the dialog's place sidebar — in
   a cafe, on a screen anyone behind the user can see.

### Architecture options

**Option A: Pass restrictive options to existing FileDialog.**
- `currentFolder: <use-case-appropriate default>`
- `options: FileDialog.DontUseNativeDialog` to force Qt's
  own impl (may or may not be faster than GTK's)
- Dialog opens at the right place; user CAN still navigate
  to network mounts but doesn't see them by default
- Doesn't fix privacy concern fully
- Cheapest fix. **S**.

**Option B: Custom Qt-native picker, app-controlled.**
- Built in QML with our own directory listing
- Local filesystem only by default
- Skips `/run/user/*/gvfs/`, network mounts, MTP devices
- User can type a path manually if they need it
- Renders fast; no GVFS probe ever
- More code. **L**.

**Option C: Permission-confirmation dialog (no picker by
default).**
- Instead of opening a picker, show:
  ```
  web2qml would like to access your music folder at:
  /home/user/Music
  
  This folder contains: 142 audio files, 8 subdirectories
  
  Grant access? [yes / no / pick different folder]
  ```
- Most users tap yes; no picker shown, no GVFS probe
- "Pick different folder" branches to Option B picker
- Best for privacy (user sees what's being granted, no
  generic file browser)
- Best for cafe-wifi (default path means no enumeration)
- Best UX (fewer clicks for the common case)
- Most work. **M-L**.

### Recommended phasing

- [ ] **FP1 — Immediate fix (Option A).** Pass
  `currentFolder` and `DontUseNativeDialog` to every
  FileDialog usage in the app. Audit all sites:
  - Music folder permission (if/when sing-challenge arc
    starts requiring it)
  - File upload dialogs
  - Save-as dialogs
  Set sensible defaults per use case. **S**.

- [ ] **FP2 — Network-mount detection and filtering.**
  At Qt level, the FileDialog's "places" / sidebar can
  include custom entries. Could also POST-PROCESS Qt's
  output by checking each path against a known-network-
  prefix list (`/run/user/*/gvfs/`, `/mnt/network/`,
  etc.) and skipping. Requires either custom dialog
  (Option B) or intercepting Qt's. **M**.

- [ ] **FP3 — Permission-confirmation dialog (Option C).**
  Replace generic FileDialog with a use-case-specific
  permission grant UI. Shows default path, folder
  content preview, three-choice action. Falls back to
  custom picker for "different folder" branch. **M-L**.

- [ ] **FP4 — Custom Qt-native picker (Option B).** Built
  in QML, local-only by default, with explicit toggle for
  "show network mounts." Replaces system FileDialog
  everywhere. **L**.

### Sequencing rationale

FP1 fixes the immediate cafe-wifi slowness with minimal
code. Files this as ship-ready and unblocks the user.

FP2 adds privacy/filtering as a follow-up.

FP3 is the real UX improvement; ships when there's
appetite for it. The permission-grant UX pattern is
useful even outside this specific bug.

FP4 is the most complete version but only worth it
once we have multiple sites that need custom picking
behavior. Single-site needs are satisfied by FP3.

### Dependencies

- FP1 is fully standalone; could ship next.
- FP2 builds on FP1's audit.
- FP3 depends on having a permission-grant UI primitive
  that we don't currently have.
- FP4 is independent but mostly redundant if FP3 ships.

### Honest scope check

The cafe-wifi report is genuinely actionable. FP1 alone
addresses the slowness symptom. FP2 addresses privacy.
FP3 addresses the deeper UX. FP4 is over-engineering
unless real use surfaces multiple needs.

Most likely sequence: FP1 next time the user hits a slow
picker → live with it → reassess if it happens often
enough to warrant FP2 or FP3.

Currently no FileDialog use is on a critical path. The
music folder permission isn't shipped yet (depends on
sing-challenge arc). File picker for uploads is rare. So
FP1 isn't urgent — file it and prioritize against the
other M-and-S items as needed.

## Corpus visibility arc (v0.1.230)

The voice corpus has been written to disk since v0.1.192 but
no UI surfaced it. Recordings accumulated invisibly. Started
addressing in v0.1.230 with a read-only listing view.

- [x] ~~Sub-ship A: read-only listing.~~ Landed v0.1.230.
  New `corpus_recorder::list_recordings()` scans
  `~/.config/web2qml/corpus/*/<unix_ms>.{wav,txt}`, reads WAV
  headers for duration, pairs `.txt` for paragraph text.
  `list_recordings_markdown()` formats as a per-speaker
  table with "When | Duration | Paragraph" columns. New
  `web2qml://corpus` virtual URL routed through manpages
  rendering pipeline (regenerated on each navigation; no
  page-cache). Quick-settings drawer has a "view recordings"
  entry. manpages.md index links to it under "Virtual pages."

- [ ] Sub-ship B: replay. Add a per-row "play" tap-target
  that shells out to a system audio player. Probably `aplay`
  on Linux (Asahi Fedora), `afplay` on macOS, `play` from
  sox as a fallback. Need to think about whether to do this
  in-process via cpal (more control, more code) or shell-out
  (simpler, depends on system tools).

- [ ] Sub-ship C: delete + relabel. Per-row delete button
  with confirm. Relabel = move the recording's file from
  `<old_speaker_id>/` to `<new_speaker_id>/`. The relabel
  case wants a picker showing all known speaker_ids from
  speakers.toml.

- [ ] Sub-ship D: corpus-driven views. Once recordings have
  speaker labels, the existing speaker-id-lookup migration
  becomes more useful — we can show "spk_alice has 23
  recordings totalling 4 minutes" in the speakers panel.
  Coordinate with the speaker-id-lookup migration when both
  are ready to land.

## Morse-via-tongue-clicks input arc (user direction, v0.1.232)

User raised: "could we add clicking the tongue as morse code
somehow... maybe an advanced user can set it up somewhere in
a web2qml://languages so that it is calibrated very well to
them."

This is a serious accessibility-adjacent input modality.
Tongue clicks have a distinctive acoustic signature (sharp
transient ~30-80ms, broadband, low DC offset) and Morse is a
well-defined symbolic system. Hands-free text/command input
for users who can't or don't want to speak words.

Multi-month arc. Not blocking; not picking up this round.
Filing the structure so future work can pick a phase.

### Why this is non-trivial

- **Transient detection != VAD.** Current voice.rs uses RMS+ZCR
  for sustained speech. Click detection needs onset-strength
  via something like a high-pass filter + spectral flux, with
  refractory period to avoid double-counting one click.
- **Calibration per-user.** A user's "dot" (short click) might
  be 40ms; another's "dash" (long click) might be 120ms.
  Without per-user calibration, the decoder is hopeless. The
  calibration UI is more work than the decoder.
- **Disambiguation from coincidental sharp sounds.** Typing,
  ambient clicks, knocks. Need a confidence threshold and
  probably a "click mode active" toggle so the system isn't
  trying to decode ambient noise.
- **Decoder latency.** Morse requires waiting for a gap to
  commit a letter. Word boundaries are even longer gaps.
  Real-time-ish UX needs careful state machine + visible
  partial decode (display "dot dot" as user emits it, commit
  letter on letter-gap).
- **Output binding.** Decoded letters → where? Typing into
  the URL bar / chat field / current focus? Triggering
  commands (Morse `O` = "open this link")? Need a target
  selection model.

### Proposed phases

- [ ] **A — Click-detection feasibility.** Add a new
  detector to voice.rs (or a new module) that processes the
  audio stream and emits ClickEvent { onset_ms, duration_ms,
  strength }. Off by default; behind `WEB2QML_CLICK_DEBUG=1`
  for inspection. Goal: characterize what real clicks look
  like from the user's mic before designing decoder.

- [ ] **B — Click calibration page.** New virtual manpage at
  `web2qml://languages/morse` (or similar). UI walks the
  user through: emit 10 dots, emit 10 dashes, emit 5 word-
  gap pauses. Stores the per-user thresholds in
  `~/.config/web2qml/morse_calibration.toml`. Same general
  pattern as the v0.1.219 voice registration flow.

- [ ] **C — Morse decoder state machine.** Given a stream of
  ClickEvents and the calibration, produce decoded characters.
  Standard ITU Morse. State machine with three timeouts: 
  intra-character (waiting for next dot/dash), inter-character
  (commit letter), inter-word (commit word + space). Visible
  partial-decode overlay so user sees their input building.

- [ ] **D — Output binding.** Decode to active focus by
  default. Special escape sequence (e.g. `....` `..` for
  "command mode") to enter a Morse command dictionary —
  short codes for common actions (open, back, scroll).
  
- [ ] **E — `web2qml://languages` umbrella.** The user's
  framing implies an "input languages" concept broader than
  Morse — could include other gesture/click/voice modalities
  as siblings. The languages page is the configuration UI
  surface for all alternative input methods. Defer until
  there's a second modality to fit alongside Morse.

### Honest scope check

Phase A alone (click detection + diagnostic) is the right
first ship if/when this arc is picked up. It's contained,
testable on hardware, and produces data the calibration UI
needs. Phases B-D depend on A working. Phase E is design
work — the umbrella concept of "input languages" might
warrant a brief design doc before code.

## Mobile unified bar arc (user direction, v0.1.232)

User raised: "could we merge the bottom tab bar and the url
bar on mobile (detect touchscreen and portrait)... this
would make hold down on url bar open up web tab and terminal
tab as options + settings (put more space for unified bar
that shrinks height by (15% * 1.5 factor) when taps or
scrolls elsewhere like in terminal or web/reading area)" and
"we could also have the back and forward arrows unified and
popups could shrink url bar left and expand on right to show
our dialog."

Multi-piece UX redesign for mobile/portrait usage. Not
shipping in full this round but the layout-mode foundation
should land soon so individual pieces can hang off it.

### Pieces

- [x] ~~**A — Compact-layout-mode property.**~~ Landed v0.1.232.
  `ApplicationWindow.compactLayout` (readonly bool) on root:
  true when `width < 720` OR portrait orientation
  (`height > width`). Override knob:
  `compactLayoutOverride` (default -1 = auto, 0 = force off,
  1 = force on) for testing. Component.onCompleted logs the
  initial detection; onCompactLayoutChanged logs transitions
  (rotation, window resize). No layout consumers yet — that
  comes in sub-ships B-E. The flag is the foundation.

- [x] ~~**B — Unified back/forward.**~~ Landed v0.1.435. In
  compactLayout the two arrows merge into one 60px pill with two
  tap zones (independent dim on can_go_back/forward), divider
  between. Wide layout keeps the two separate ToolBtns.

- [ ] **C — Unified URL+tab bar (compact mode only).** In
  compactLayout=true:
  - The bottom Browse|Terminal tab bar disappears
  - The top URL bar becomes the sole horizontal strip
  - Long-press on URL bar reveals a context menu: switch to
    Browse tab / switch to Terminal tab / Settings
  - URL bar gets extra horizontal space (previously shared
    with chrome elements)
  In compactLayout=false (desktop / landscape / wide):
  - Existing dual-bar layout unchanged

- [ ] **D — Bar height transition.** When user taps or
  scrolls in terminal or reading area, the unified bar
  shrinks vertically by ~22.5% (15% × 1.5 factor per the
  user spec). On tap-back in the bar area, restores. Smooth
  animation on height property. Need to think about which
  tap events trigger shrink vs which are inside the bar
  itself (don't shrink when user is tapping the URL bar to
  edit).

- [ ] **E — Popup reflow.** When a modal-style popup
  appears, the URL bar shrinks left (reduces width) and the
  popup expands rightward into the freed space. Avoids the
  popup hovering over the URL bar (currently popups overlap
  or get pushed). Needs anchor changes on every popup that
  positions relative to the URL bar.

### Suggested ship order

1. **A (compactLayout property)** — foundation, no behavior
   change yet
2. **B (unified back/forward)** — smallest visual change,
   ships independently
3. **C (unified URL+tab bar)** — biggest single change,
   ships once A is in
4. **D (height transition)** — polish on top of C
5. **E (popup reflow)** — touches every popup; ships last
   when the layout is stable

### Risk notes

- **Long-press vs context menu vs tap-and-hold.** Qt's
  TapHandler has `onLongPressed`. The pattern is already
  used in v0.1.197+ for column header taps. Reuse.
- **Touch detection.** QInputDevice::devices() can be queried
  for capability::Touch. But Asahi Fedora may not always
  report touch correctly. Width+portrait is the more
  reliable signal. Compact-layout-mode should NOT depend on
  perfect touch detection; failing-open to "wider layout"
  is safer than failing to "compact" on a desktop.
- **Discoverability.** A hidden long-press menu is a
  feature-discovery problem. The compactLayout mode should
  surface SOME hint (e.g. a small downward-chevron next to
  the URL bar) so the user knows there's more there.

## In flight (current arc)

### v0.1.241 reading-overlay feedback (user session, May 2026)

User feedback after testing the reading overlay against
real documents:

> "for code blocks as reading we need tree-sitter-rs to help
> us pace words they are going too fast right now"

> "also the popup is not interleaving itself into the page
> and emerging where the header is... reading at the bottom
> of the screen is going to cause the reader to lose focus
> on their knowledge and turns the savings back into a chore"

> "the highlighted words should also be centered vertically
> for things that expand beyond the bottom border because
> otherwise you can't see them... this is why teleprompters
> scroll"

**Landed in v0.1.241:**
- [x] Overlay moved from bottom to JUST BELOW the URL bar
  (`anchors.top: topBar.bottom`). Eye doesn't have to
  shuttle between the paragraph and a bottom strip.
- [x] Teleprompter-style scroll for the word display.
  Words live in a clipped viewport (~140px tall) and the
  Flow's y is computed so the current word's vertical
  center aligns with the viewport's vertical center.
  300ms NumberAnimation Behavior smooths the scroll
  between word advances.
- [x] Bigger overlay (220px in reading state, was 140)
  to give the teleprompter viewport room to breathe.
- [x] Opacity floor 0.1 (was 0.0) so surrounding context
  words stay faintly readable instead of disappearing.
  Window expanded from ±2 to ±5 words at progressively
  dimmer levels.
- [x] Slider popup repositioned: was `y: -height - 8`
  (above the bottom-anchored overlay), now
  `y: readingOverlay.height + 8` (below the top-anchored
  overlay).

**Still open (this batch):**

- [ ] **M+ — Tree-sitter pacing for code blocks.** Reading
  rate is uniform per-word right now. Code reads slower
  than prose — punctuation pauses (`;`, `,`, `(`), longer
  pauses for newlines/dedents, identifier syllable
  rhythm. User: "code blocks are going too fast right
  now."
  
  **v0.1.241 conversation update — superseded by phoneme arc.**
  This original tree-sitter approach has been folded into
  the Phoneme alignment + tiered reading model arc filed
  earlier in this document. Specifically:
  
  - **Phoneme arc PA1** addresses code-too-fast with the
    cheapest possible fix: per-content-type multiplier
    (code = ~0.7x of prose WPM). No tree-sitter needed
    for that.
  - **Phoneme arc PA4-5** brings true phoneme-aligned
    advance which handles code timing as a natural
    consequence (the user's actual speech timing drives
    advance, not a uniform timer).
  - Tree-sitter would only add value if we wanted per-
    TOKEN pacing within code (e.g., longer pause after
    `;` than after a variable name) BEYOND what PA4-5
    already provides through real speech alignment.
  
  Decision: don't pursue tree-sitter as a separate arc.
  PA1 handles the immediate "code is too fast"
  complaint with config values. PA4-5 handles the
  fundamental "uniform timer is wrong" issue. Tree-
  sitter could come back as a refinement of PA1 IF the
  data shows that per-token pacing inside code would
  help beyond what PA4-5 gives us. Treat as filed-and-
  superseded.

- [ ] **L+ — Inline word highlight on the paragraph
  itself.** The teleprompter overlay solves the eye-shuttle
  problem partially (overlay is up near header instead of
  at the bottom) but the most-honest version is to
  highlight the word RIGHT WHERE IT LIVES in the page
  content — no separate display surface at all.
  
  Approach: while reading, the selected paragraph's Text
  element switches to RichText mode with each word in a
  span. Per WPM tick, regenerate the rich text with the
  current word at full opacity, others dimmed. Also
  auto-scroll the page so the current word stays in
  view.
  
  Risk: setting `text:` on a Text element with rich
  content triggers re-layout. At 200 WPM that's ~3 times
  per second; doable but heavy. The wrapper Item would
  also need to communicate state to readingOverlay
  bidirectionally.
  
  Defer until the teleprompter overlay validates the
  approach. If teleprompter is "good enough" the inline
  version might not be needed; if it's STILL a chore
  for long reads, then the inline rewrite is worth it.


### v0.1.241+ hardware-test feedback (user session, May 2026)

Accumulated during an extended architecture-design
conversation following the v0.1.241 reading-overlay
redesign ship. Items below were reported but NOT shipped
in v0.1.241 — they're the "immediate queue" pending
real code work.

**The discipline issue worth naming:** these items have
been deferred across many turns while design work happened
on the phoneme alignment arc, terminal performance options,
folder picker, etc. The architecture filings are valuable
but the user-facing experience hasn't improved during that
period. When ship cycles resume, these items should land
before any speculative architectural work.

**Bugs (real breakage):**

- [ ] **M — Terminal scroll cuts commands at window edges.**
  Scrolling up with full history clips commands not aligned
  to viewport boundaries. Likely a row-snapping issue in
  the terminal renderer. Need to check pty_term.rs scroll
  math — possibly an off-by-one in the visible-row
  calculation when scrollback is positioned mid-row.

- [~] **Entered commands appended with newline.** v0.1.457
  fix attempt: termKeyArea had TWO CR-on-Enter paths (Keys.onPressed
  + onTextChanged IME commit); on Asahi/Wayland a single Enter can
  fire both → double CR → blank line. Both now route through
  _sendEnterOnce() (30ms timestamp dedup, below auto-repeat). Safe
  even if hypothesis wrong (sends once if only one fires). AWAITING
  hardware confirm; if still broken it's shell-side vim-mode. User:
  "entering commands that are supposed to have just been
  entered are just appended with a newline I wonder if is
  my shell behavior with vim motions or not." Could be
  shell-side (zsh/bash vim mode multiline edit) OR our key
  handling. Needs diagnosis pass first:
  - Reproduce: run in plain bash without vim-mode → does
    it still happen?
  - If yes: our terminal is inserting unintended newlines
    on enter
  - If no: shell-specific, file separately as guidance
  - Check pty_term.rs key event handling for enter key —
    is it sending `\r` or `\r\n`?

**Visual fixes (small):**

- [x] ~~**S — Selection box color contrast across themes.**~~
  **Landed v0.1.243.** New `selection_accent` field on
  RenderStyle: dark page → `#ffaa44` warm orange, light
  page → `#c04020` dark red-brown. Distinct from link_color
  and text_color, saturated for contrast against page bg.

- [x] ~~**M — Selection box visual upgrade with animations.**~~
  **Landed v0.1.243 (partial).** Shipped breathing opacity
  (0.7 → 1.0 → 0.7 over 3s ease-in-out) and parallax-lite
  (2px x-offset on hover, 300ms ease). Held off on skew
  and bulge — file separately if breathing+parallax aren't
  enough.

**Reading rate fine-tuning:**

- [x] ~~**S — Lower default WPM by 5%.**~~ **Subsumed by PA1
  v0.1.242.** PA1's prose=0.95 multiplier brings effective WPM
  to 152 at the 160 base, which is the same as the requested
  5% drop. No separate ship needed.

- [x] ~~**M — Markdown tables overflow viewport, forcing
  horizontal page scroll.**~~ **Landed v0.1.243.** Fixed via
  Qt RowLayout idiom: cells get `Layout.preferredWidth: 1`
  alongside `Layout.fillWidth: true`. Forces equal width
  distribution; wrapMode kicks in to wrap long content
  within cells. Applies to both webpage and manpage table
  paths (they share emit_adaptive_table). The deeper
  options (truncate-and-tooltip, in-table scroll, vertical
  stack) remain available if some edge case still
  overflows; revisit if real use surfaces a failure.

### v0.1.244 hardware test feedback

App opening sanity check passed: "basic app opening is good"
after hardware test of v0.1.244 build (cargo.lock confirms
v0.1.240 → v0.1.244 jump). User found a small focus-handler
bug locally and patched it in-tree:

```
--- onActiveFocusChanged: {
---     if (activeFocus) urlBarVisible = true
--- }
+++ // (commented out — moved into the other onActiveFocusChanged)

--- onActiveFocusChanged: { if (activeFocus) selectAll() }
+++ onActiveFocusChanged: { if (activeFocus) { urlBarVisible = true; selectAll() } }
```

The user merged the urlBarVisible side-effect into the urlBar's
single onActiveFocusChanged handler, avoiding what appears to
have been a double-handler conflict. Worth landing this pattern
upstream (TBD: confirm the user wants me to merge their patch or
leave it as their local hack).

**Two new issues surfaced from the same session:**

- [x] ~~**M+ — URL unified bottom bar not yet implemented.**~~
  **Landed v0.1.249 (partial — moved, not fully consolidated).**
  URL bar moved from top to bottom of layout, sits just above
  the existing tab bar. Reading overlay re-anchored to
  parent.top. Link hint Y offset adjusted. The "unified" part
  of the original spec (combining URL+nav+tab+mic into ONE
  bar) deferred — would need more UX design about button
  placement and tab switching. Moving accomplishes the
  primary motivation (clear top zone, thumb reach).

- [ ] **M — URL+tab bar consolidation (follow-up to v0.1.249).**
  After v0.1.249, the bottom of the window has TWO stacked
  bars: URL bar (50px when visible) above tab bar (48px).
  Total ~98px when URL visible. The original "unified bottom
  bar" spec wanted these merged into one bar.
  
  Open UX questions to resolve before shipping:
  
  1. **Where do back/forward live?** Currently in the URL bar's
     RowLayout. If merged into a single bar, they need a place.
     Options:
     - Stay in URL bar row, tab toggles become smaller siblings
     - Move to a "navigation cluster" at one edge of the bar
     - Become gesture-only (swipe edge)
     - Hide unless URL bar has focus
  
  2. **Where does the drawer button (☰) live?** Currently leftmost
     in URL bar. Could stay or move to bar's edge.
  
  3. **Tab switcher form factor?** Currently a two-button row
     (Browse / Term). In a unified bar, this needs less space.
     Could become a SMALL toggle, or just a "switch" gesture
     (swipe up on the bar = open switcher).
  
  4. **What's the visual hierarchy?** Right now URL bar visually
     dominates (it's wide and pill-shaped). In a unified bar
     where tab toggle is also visible, both compete. Probably
     the URL pill stays the primary visual element with smaller
     siblings around it.
  
  Probably wants a small mockup/sketch before code. M scope
  once design is settled.

- [ ] **M+ — Touch-drag scroll in helix prints `<5`/`<6`
  characters (NOT scrolling).** User comment: "scrolling
  with touch drag in helix still starts printing <5 and <6
  characters.. qt rendering bug + not wired up to scroll
  backend that helix has?"
  
  **Investigation note from v0.1.244 audit (without hardware
  reproduction):**
  
  Our touch-drag handler in `qml/main.qml` (terminal MouseArea,
  ~line 2867) DOES handle scroll correctly in TUI mode:
  emits Ctrl-U (0x15) for drag-down and Ctrl-D (0x04) for
  drag-up via `backend.pty_write_byte()`. Threshold 12px,
  step 3*charH or 42px.
  
  The bytes 0x15 / 0x04 would NOT print as `<5`/`<6` if the
  PTY received them correctly. So the `<5`/`<6` characters
  the user sees are NOT coming from our scroll emit.
  
  Strongest hypothesis: **helix has enabled SGR mouse
  tracking** (sends `\x1b[?1006h` at startup). Our pty
  layer doesn't currently track or respond to mouse-mode
  CSI sequences (grep confirms no mouse_tracking state in
  src/*.rs). So when the user touches the terminal pane,
  Qt's underlying event path may be writing raw mouse
  events to PTY in SGR format `\x1b[<5;col;row M` (button
  code 5 = wheel up SGR, 6 = wheel down). Helix RECEIVES
  these correctly... but the leading `\x1b[` may be eaten
  somewhere, leaving the visible `<5`/`<6` characters in
  the buffer.
  
  **Diagnosis steps for next hardware session:**
  
  1. Reproduce: open helix on a multi-line file, touch the
     screen and drag vertically. Confirm `<5` / `<6` appear
     as visible text in the buffer.
  2. Check if helix specifically: try the same in vim. If
     vim doesn't reproduce, helix-specific mouse handling
     is implicated.
  3. Check our scroll path: add a temporary
     `eprintln!("[pty_write] {:02x}", b)` in pty_term.rs's
     pty_write_byte. Touch-drag and check terminal stderr
     for the bytes actually being written.
  4. Check Qt's native path: if our pty_write_byte ISN'T
     firing during touch-drag but `<5`/`<6` STILL appear,
     they're coming from a Qt-internal path (TextEdit's
     own mouse handling forwarding to focus item, or
     ApplicationWindow event filter).
  
  **Likely fix path:**
  
  If diagnosis confirms Qt is forwarding raw mouse events:
  intercept at the QML level by setting `acceptedButtons`
  on the touch-handling MouseArea to a broader set, AND
  set `propagateComposedEvents: false` so no other
  handler/Qt-internal can fire.
  
  If helix's mouse tracking is needed (it gives nice
  features like click-to-place-cursor):
  - Add mouse-mode state tracking to pty_term.rs
    (parse `\x1b[?1006h` etc., track active mode)
  - In our touch handler, when alt-screen + mouse-mode
    enabled, translate touch-drag deltas to SGR wheel
    events: `\x1b[<64;<col>;<row>M` for wheel up,
    `\x1b[<65;<col>;<row>M` for wheel down. NOT
    `<5`/`<6` (those are X10 button codes, deprecated).
  - Helix would then receive proper wheel events and
    scroll naturally.
  
  Touchpoints:
  - `qml/main.qml` terminal MouseArea (verify event capture)
  - `src/pty_term.rs` (add mouse-mode state if pursuing
    the wheel-event translation path)
  - Hardware reproduction needed before any code lands
  
  Scope: S (if Qt event capture fix is enough) to L (if
  full mouse-mode tracking is needed for SGR wheel events).

### v0.1.247 audit findings (no hardware test, code-only review)

Findings from the v0.1.247 cleanup audit pass. Not shipped —
filed as observations for future ships where appropriate.

- [x] **S — Headings use ContentType::Prose, but headings are
  a different reading register.** **Landed v0.1.306.** Added
  `ContentType::Heading` variant with `as_qml_str() = "heading"`.
  Both call sites updated (qml_gen.rs h1/h2/h3 emit + manpages.rs
  level 1-3 emit). QML's `_baseMultiplierFor` returns 0.80 for
  "heading" (20% slower than prose's 0.95). ContentType test
  fixture extended to cover the new variant.

- [x] **S — manpage prose paragraphs wrap unconditionally,
  bypassing word-count bounds.** **Landed v0.1.299 (option 1).**
  `src/manpages.rs:emit_paragraph_qml` now checks
  `qml_gen::paragraph_word_bounds()` (the same 10-50
  default that webpage `<p>` uses) BEFORE wrapping with
  `wrap_for_paragraph_selection`. Out-of-bounds paragraphs
  render as plain Text without the selection wrapper. New
  `qml_gen::approx_word_count_pub` and `paragraph_word_bounds`
  visibility bumped to pub(crate) so cross-module use is
  supported. Manpage paragraphs are now consistent with
  webpage prose for selection eligibility and phoneme-db
  coverage.

- [x] **Defensive empty-string handling in paragraph_selected
  is dead code.** **Landed v0.1.287 (test fixture pinned).**
  Added two unit tests to `src/qml_gen.rs` pinning the
  contract: `content_type_variants_are_non_empty` asserts
  every ContentType variant maps to a non-empty literal,
  and `content_type_string_values_stable` asserts the exact
  strings ("prose", "code", "list", "table"). If a future
  variant is added with an empty string OR a typo
  ("proses"), CI catches it before the defensive empty-string
  defaults in main.rs/main.qml silently disagree.

- [x] ~~**v0.1.243 selection_accent collides on 3 themes.**~~
  Fixed v0.1.422. Added RenderStyle::with_theme_idx(idx) (chained
  in Backend::style) overriding selection_accent for the colliding
  indices — ReadLt(9)->#1f7a6a, Sepia(10)->#1a6b8a, Amber(14)->
  #33ccbb (cool hues away from each theme's warm tc/lk). No
  from_theme signature change, so zero callsite churn. Indices
  verified against the QML themes array.

- [x] **trivial — Stale "pre/code not yet selectable" docs in
  TWO places.** **Landed v0.1.287.** Both sites updated to
  reflect v0.1.244's wiring of pre/code blocks through
  maybe_wrap_extended. The 0.70 multiplier doc now reads
  "v0.1.244 wired <pre>/<code> blocks through
  maybe_wrap_extended so they ARE now selectable; this
  multiplier applies" in both `src/qml_gen.rs` and
  `qml/main.qml`.

- [x] ~~**S — Cargo opt-level is "z" (size); investigate whether
  "s" or "3" affects perceived runtime perf.**~~ Done v0.1.882:
  switched `[profile.release]` `opt-level` from `"z"` to `3` (the
  Rust release default). `"z"` aggressively optimizes for binary
  SIZE at the cost of runtime speed — unusual for a desktop Qt app
  and a plausible contributor to the terminal-feels-slow-vs-Konsole
  reports. With `lto = true`, `codegen-units = 1`, and `strip = true`
  already set, the binary stays lean while the Rust hot paths (grid
  diffing, parsing, render) run at full speed. Original note:
  
  ```
  [profile.release]
  opt-level = "z"
  ```
  
  "z" optimizes aggressively for binary SIZE at the cost of
  runtime speed. Typical contexts: embedded, wasm. Less
  common for desktop Qt apps. Given user's reports about
  terminal performance feeling slow, the opt-level might be
  contributing to the gap vs Konsole.
  
  Test plan: build three variants of the same git revision:
    - `opt-level = "z"` (current)
    - `opt-level = "s"` (moderate size opt, faster)
    - `opt-level = 3`  (full speed)
  Compare:
    - Binary size on disk
    - Startup time (cold cache, time-to-first-frame)
    - Terminal scroll perf (running the troubleshooting.md
      profile recording protocol)
    - QML parse time
  
  If "s" or "3" is meaningfully faster AND the size penalty
  is acceptable (probably an extra few MB on top of a binary
  that's already 10+ MB after all the bundled models), it's
  a free perf win.
  
  Should be done in conjunction with TP4 (Konsole-baseline
  profiling). Don't change without measurement.

- [ ] **S — panic = "abort" means Backend's Drop never runs on
  crash; data loss on panic.** From v0.1.247 audit pass 3:
  
  ```
  [profile.release]
  panic = "abort"
  ```
  
  `Backend::drop` saves palette, archive, settings, and (as of
  v0.1.287) corpus writer + (v0.1.289) acoustic_index. With
  `panic = "abort"`, a panic skips all Drop handlers. Any
  changes made since the last explicit save are lost.
  
  Switching to `unwind` would:
    - Run Drop on panic, preserving the saves
    - Add ~100-300 KB of unwinding tables to the binary
    - Make panics in audio thread (cpal callback) marginally
      more complex (need to ensure panics there are caught
      rather than poisoning the audio backend)
  
  **Mitigation progress** (the cheaper "save eagerly" approach):
  - v0.1.286: archive saves on every mutation (was: batched 5)
  - v0.1.286: palette saves on every add (was: only Drop)
  - v0.1.287: corpus writer thread JoinHandle tracked +
    flushed in Drop (clean shutdown only; abort still loses
    in-flight write)
  - v0.1.289: acoustic_index debounced save every 5
    mutations + Drop flush
  - settings: still saves only via explicit `save_settings`
    qt_method calls from QML (debounced timer + explicit
    triggers). Multiple call sites in QML hit it; should be
    covered for normal use.
  
  **Still on the abort-panic risk surface**: anything that
  mutated state since the last debounce-window save.
  Acceptable for now; revisit if a hardware panic event
  reveals a specific data loss.

### Shell-aware NLP + lexicon arc (filed v0.1.249 conversation)

User direction:
> "what about adding nushell nlp defaults and lexicon... it
> really is a different shell... i would also argue we should
> have some basic python interpreter nlp as well"

Today the only NLP path is `table_nlp.rs` (parses commands like
"show columns 1 4 7" into table ops). It assumes the user is
manipulating a rendered table.

This arc extends the same approach to the terminal pane: when
the user types/speaks a natural-language command and they're at
a SHELL prompt (or Python REPL), translate to the appropriate
shell's idioms.

**Why this matters:** voice-first navigation is the core
thesis. If voice commands always compile to `bash` and the
user is in `nu` (which is a fundamentally different shell —
structured data pipelines, not text streams), voice becomes
useless and the user falls back to typing. The translation
layer must be context-aware about the active interpreter.

**Architectural sketch:**

```
src/
  shell_nlp/
    mod.rs           - dispatch by detected interpreter type
    common.rs        - vocabulary shared across all targets
                       (file/path/text concepts)
    bash.rs          - bash + sh + zsh fallback
    nu.rs            - nushell idioms (structured pipelines)
    fish.rs          - fish (similar to bash but `set` semantics)
    python.rs        - python REPL / interactive (expressions)
    ipython.rs       - python REPL + magic commands
  shell_detect.rs    - identify the foreground interpreter
```

**Lexicon format (TOML, user-extensible, same pattern as
data/phonetic_confusions.toml):**

```toml
# data/shell-lexicon/nu.toml
[concepts.list_files]
patterns = ["show files", "list files", "what's here", "ls"]
template = "ls"
variants = [
  { when = "with sizes", template = "ls --long" },
  { when = "largest",
    template = "ls | sort-by size --reverse | first 10" },
  { when = "recent",
    template = "ls | sort-by modified --reverse | first 10" },
]

[concepts.filter_text]
patterns = ["filter", "grep", "find lines with"]
template = "where $it =~ \"{query}\""
notes = """
  Nushell uses regex matching on $it rather than bash pipe-
  to-grep. Strings, not bytes.
"""
```

Same concepts, different `template` per shell. Dispatcher reads
the active shell's lexicon and produces the command the user
expects.

**Sub-ships (rough sequence):**

1. **SNLP-1 — Shell detection (prerequisite).** Identify the
   currently-running interpreter in the terminal pane. Three
   approaches in order of robustness:
   - Parse PS1 prompt structure (fragile — users customize)
   - Read /proc/<pty_pid>/comm for the foreground process
     (Linux-specific but reliable; we already do similar for
     password-prompt detection)
   - Heuristics on recent output (look for nu's banner, python's
     `>>>`)
   
   Probably option 2 with option 3 as fallback. Expose as a
   qt_property `foreground_interpreter: QString` ("bash", "nu",
   "python", "ipython", "unknown"). 100-200 lines in
   pty_term.rs. **S-M**.

2. **SNLP-2 — Lexicon loader.** Read `data/shell-lexicon/*.toml`
   at startup. Same pattern as `data::init()` for
   phonetic_confusions. Expose `lexicon_for(interpreter)`. **S**.

3. **SNLP-3 — Bash lexicon (baseline).** Cover the 20-30 most
   common concepts: list/filter/sort/count/find/edit/move/copy/
   remove. Lexicon-only, no Rust changes after SNLP-2. **S**.

4. **SNLP-4 — Nu lexicon.** Same concepts, nu idioms. The
   interesting part is that nu's structured-data pipelines
   differ from bash both syntactically AND semantically — for
   "show me JSON from this command", nu pipes the parsed value
   directly, bash pipes text. Some concepts get richer
   variants for nu. **M**.

5. **SNLP-5 — Fish lexicon.** Largely a bash variant; same
   concepts with small syntax tweaks (`set -l` vs `local`,
   etc.). **S**.

6. **SNLP-6 — Python REPL lexicon.** Python is fundamentally
   different — it's an expression evaluator, not a shell.
   Initial scope: list comprehensions, basic file ops via
   pathlib, common pandas/polars/dict patterns IF the user has
   imported them. Detect imports from recent input (we have
   the scrollback). Don't try to be general; cover 10-15
   high-value patterns. **M-L**.

7. **SNLP-7 — iPython extras.** Magic commands (`%timeit`,
   `%load`, `%ls`). Detect "I'm in ipython not plain python"
   via banner or first prompt. **S** after SNLP-6.

8. **SNLP-8 — NLP-to-template matching upgrade.** Today
   table_nlp uses regex + token walking. Shell NLP needs to
   handle phrasing variation better. Options:
   - Hand-rolled with confidence scores (cheap, predictable)
   - Local LLM call (expensive, more flexible)
   - Phoneme-aware match via PA4-7 (very late)
   
   Start with hand-rolled, file confidence-score and LLM as
   upgrades. **M-L**.

**Architectural decisions to think through:**

- **Should the URL bar accept shell commands too?** Currently
  it accepts URLs and search queries. Adding "if you type
  something on terminal tab, route to shell NLP" would
  unify input. But the URL bar visually belongs to the browse
  pane. Maybe a separate command bar that appears on terminal
  tab? Or a "smart paste" gesture?

- **Voice-first means the NLP layer needs to handle
  recognition errors.** Whisper output "find all puffies" vs
  "find all .py files" — the lexicon should know that
  "puffies" is likely "py files" in a coding context. Maybe
  the lexicon TOML has phonetic-confusable patterns too,
  reusing the phonetic_confusions.toml infrastructure.

- **Conflict between table_nlp and shell_nlp.** A user types
  "filter column 1 > 50" — table_nlp grabs it. They type
  "filter csv where column 1 > 50" in terminal — shell_nlp
  should route. Need clear precedence rules and probably
  visible feedback ("I think you mean: [bash] awk -F, '$1>50'
  data.csv — confirm?").

- **Lexicon versioning.** Nu changes syntax across versions.
  A `min_nu_version` field on each concept or variant lets
  us avoid emitting deprecated forms. Bash is more stable but
  POSIX vs GNU vs zsh-as-bash matters.

- **User-extensible lexicons.** TOML in `~/.config/web2qml/
  shell-lexicon/*.toml` overrides bundled defaults. User's
  personal aliases ("ll" → "ls -la") become NLP-addressable.
  Probably the killer feature of the whole arc.

**Sequencing:** SNLP-1 + SNLP-2 + SNLP-3 are the minimum
viable. Without SNLP-1 we can't dispatch correctly; without
SNLP-2 the lexicon doesn't load; without SNLP-3 there's
nothing to demonstrate. Probably ship those three together
as one focused arc-opener.

SNLP-4 (nu) is where the conceptual win shows up — nu's
structured pipelines is the reason the lexicon exists at all.
Probably ship SNLP-4 within a couple weeks of SNLP-3.

SNLP-6 (python) is its own thing and can wait. The Python
REPL is less common in voice-first workflows than shell
work, AND the technical challenges are bigger (state
tracking, import detection, dataframe library awareness).

**Total scope:** 5-8 ships over 2-4 months if pursued
seriously. Same shape as the phoneme alignment arc — long-
horizon work with clear sub-pieces.

**Defer until:** the recent ship cascade (v0.1.241-249) is
hardware-validated and we have profile data from TP4. Both
SNLP-1 (process detection adding /proc reads) and SNLP-6
(scrollback parsing) need to be cheap; we should know what
the current performance budget looks like before adding new
recurring work.

### Reframing (v0.1.249 conversation, after initial filing)

User direction shifted the architecture:

> "i think python shouldnt fire unless an ollama model didnt
> time out or claude api is up"
> "could the nushell solution be use nix version pinning
> suggestions when a command fails to stdout or stderr?"

Two important reframings.

**Reframe 1: Python REPL NLP requires an LLM, not a lexicon.**

The hand-rolled lexicon approach works for shells because shell
commands are stateless one-liners with regular grammar. Python
REPL is fundamentally different:

- Python has state — what's imported, what variables exist
- Python uses expressions, not commands — "filter empty rows"
  isn't a one-liner unless we know `df` exists and what columns
  it has
- The right expression depends on what library is loaded
  (pandas? polars? plain dict?)

Hand-rolled lexicon dispatch can't handle this. An LLM can —
it sees recent REPL history, infers context, produces a working
expression. So **SNLP-6 (python) gets re-architected**:

- Don't try to do lexicon-based python. Won't work.
- Gate python NLP on LLM availability: if Ollama responds to
  a quick probe (`/api/tags` with 100ms timeout) OR Claude API
  key is set and reachable, python NLP is available. Otherwise
  it's unavailable.
- Failure mode: explicit "Python NLP requires LLM; none
  available — type the expression yourself." NOT a degraded
  hand-rolled fallback that's confidently wrong.

Implementation pieces:
- [x] ~~New `llm_health::available()`~~ Landed v0.1.441 (as a
  general perf gate, ahead of the python-NLP work that
  originally motivated it). `ollama list` daemon probe w/ 300ms
  timeout (not just binary-on-PATH), 30s cache, claude-key
  overlay, invalidate() on key change. Wired into the summary
  worker + voice fragment classifier to skip doomed 8s timeouts.
  The python-NLP path can consult the same gate when built.
- Python NLP path consults this gate first. If false, returns
  None and the input flows through as a normal keystroke (or
  the UI surfaces "LLM offline").
- The LLM gets a structured prompt: recent REPL output + the
  user's request + a system message about producing a Python
  expression. Probably stream the response so the user can
  cancel.

This cleanly separates the failure modes:
- Shell NLP failure: "I don't know that phrase" (lexicon
  doesn't have it)
- Python NLP failure: "I don't have an LLM right now" (offline
  / no key / timed out)

Different mitigations, different user-facing messages.

**Reframe 2: Reactive command-failure recovery is its own arc.**

What I originally filed conflated two different problems:

- **Predictive NLP**: user speaks/types natural language → we
  emit a shell command. (Lexicon-based, voice-first oriented.)
- **Reactive recovery**: user runs a command, it FAILS → we
  suggest a fix based on the error output.

These have different architecture and different value. Reactive
recovery is potentially a higher-leverage feature because
"command not found" is universal — every user hits it. The
fix vocabulary (nix install, apt install, pip install, etc.)
is well-defined.

**New arc: NIXFIX (or COMMAND-RECOVERY).**

Flow:

1. User runs `npm install` in any shell.
2. Command exits non-zero. Stderr contains "npm: command not
   found".
3. We parse the failure pattern, identify "npm" as the missing
   binary.
4. Suggest a fix inline below the command:
   - On NixOS / with Nix available: `nix shell nixpkgs#nodejs`
     (or version-pinned variant)
   - On Ubuntu/Debian: `apt install nodejs`
   - On Fedora: `dnf install nodejs`
   - On macOS with Homebrew: `brew install node`
   - Show the appropriate one based on detected package manager
5. User taps the suggestion. We send it to PTY (with
   confirmation if it requires sudo or is destructive).

The arc breaks down into sub-ships:

- **NIXFIX-1 — Failure pattern matcher.** Detect common
  failure modes from stderr/stdout:
  - `command not found` (POSIX), `Unknown command` (nu),
    `[command] is not recognized` (Windows)
  - `ModuleNotFoundError`, `ImportError` (Python)
  - `Error: Cannot find module` (Node)
  - `error[E0432]: unresolved import` (Rust)
  - `No such file or directory` (with path)
  - `Permission denied`
  Each pattern extracts the missing thing (binary name, module,
  file, etc.). New file `src/failure_patterns.rs`. **S-M**.

- **NIXFIX-2 — Package manager detection.** What's available
  on this system? Check `which nix nix-env apt dnf brew pip
  pip3 cargo npm`. Cache the result. New file
  `src/pkg_mgr.rs`. **S**.

- **NIXFIX-3 — Recovery suggestion engine.** Given (failure
  type, missing thing, available package managers), emit one
  or more suggestion strings. TOML lexicon for the mapping —
  same pattern as shell-NLP lexicon. **M**.

- **NIXFIX-4 — Inline UI for suggestions.** Where do
  suggestions appear in the terminal pane? Options:
  - Ghost-text overlay just below the prompt (after the
    error output, before the next prompt)
  - Toast at the bottom (current toast infra)
  - Highlighted block in the scrollback with tap-to-apply
  Probably option 1 with option 3 as a "history of recent
  suggestions" panel. **M**.

- **NIXFIX-5 — Apply-with-confirmation.** Tapping a
  suggestion writes the command to PTY. Suggestions that
  require sudo or are destructive get a confirm step. **S**.

- **NIXFIX-6 — Version pinning (Nix-specific).** When the
  user explicitly wants reproducibility, suggest the
  versioned variant: `nix shell nixpkgs/nixos-23.11#nodejs`
  with version locked. Nice-to-have, defer. **S**.

**Why this is potentially higher-leverage than predictive
NLP:**

- Every user hits command failures. Voice-first users hit
  them differently but still hit them.
- The intervention timing is GOOD — exactly when the user
  wants help.
- The vocabulary is BOUNDED — known failure patterns, known
  package managers. Not trying to be general.
- The Nix angle gives version reproducibility which
  predictive NLP can't.
- Cross-shell — works in bash AND nu AND fish identically.
  Doesn't depend on shell detection working.

**Revised sequencing recommendation:**

After hardware validation of v0.1.241-249:

1. **NIXFIX-1 + NIXFIX-2** (~one focused session) — failure
   pattern matcher + package manager detection. Land them
   together because they're prerequisites for each other.
2. **NIXFIX-3 + NIXFIX-4 + NIXFIX-5** (~two sessions) — the
   suggestion engine and UI.
3. **SNLP-1 + SNLP-2** — shell detection + lexicon loader.
4. **SNLP-3** — bash baseline lexicon.
5. **SNLP-4** — nu lexicon (the conceptual win).
6. **NIXFIX-6** — nix version pinning.
7. **SNLP-6 (LLM-gated python)** — once LLM health probe
   exists.

NIXFIX before SNLP because it has clearer immediate value
and doesn't depend on shell detection. SNLP-6 (python) last
because it has the most prerequisites.

**Defer for now.** All filed; ship cascade still needs
hardware validation first.

### Package index + offline mode (v0.1.249 conversation)

User direction:
> "some caching or batch searching of apk dnf or apt when
> commands fail but not all the time because we dont want to
> spam servers and cost money unless needed"
> "suggestion button for nix shell venv etc"
> "check at start for apk list or dnf list to see what might
> be avail and if worth it to check again once per day with
> batched commands"
> "adding a setting called offline mode to quick settings
> that disables claude, web traffic to sites (but markdown
> still works) and package updates"

This extends NIXFIX (reactive command failure recovery) with
caching strategy + introduces a system-wide offline mode
toggle. Architecturally three sub-arcs.

**Arc PKGCACHE: layered package index.**

Two layers of "what packages are available":

- **Bundled curated subset** (small, baked into binary):
  ~1000 commonly-missing tools (`npm`, `python`, `cargo`,
  `helix`, `rg`, `fd`, `jq`, `ripgrep`, etc.) mapped to their
  install commands across package managers. Same pattern as
  phoneme-db.txt — out-of-band generation script, checked-in
  static data, runtime lookup is instant. Covers ~95% of real
  "command not found" failures.
  
  Generator: `scripts/gen-pkg-curated.py` walks a list of
  popular tools (could start by scraping the top of e.g.
  GitHub-trending CLI tools) and queries each package
  manager's database for canonical names + install
  invocations. Output: `data/pkg-curated.txt` in a line-based
  format like phoneme-db.txt.

- **System-local full index** (refreshed daily, batched):
  cached file at `~/.config/web2qml/pkg-index.bin`. Built by
  running ONE batched command per detected package manager:
  - apt: `apt-cache pkgnames` (~1s; ~50k names)
  - dnf: `dnf list available --quiet` (~5-10s; ~30k names)
  - apk: `apk search -q` (~1s)
  - brew: read from `~/Library/Caches/Homebrew/api/formula.json`
    or `brew formulae --quiet`
  - pip: SKIP for v1 (PyPI index is huge; covers same ground
    as bundled subset for popular libraries)
  - nix: SKIP for runtime refresh (nix search is too slow).
    Use the bundled curated subset which can include a
    nixpkgs snapshot if the build environment has nix.
  
  Triggered on first NIXFIX-3 use AND staleness >24h. Runs
  in background thread, doesn't block recovery suggestions
  (uses curated subset in the meantime).

**PKGCACHE sub-ships:**

- **PKGCACHE-1**: Curated subset generator script + bundled
  data file. Same shape as PA3 (phoneme-db). `scripts/gen-pkg-
  curated.py`, output to `data/pkg-curated.txt`, runtime
  loader in `src/pkg_index.rs`. Curated list to be decided —
  start with maybe 100 tools and expand. **M**.

- **PKGCACHE-2**: System-local index loader. Reads
  `pkg-index.bin` if present, otherwise falls back to
  curated. Detects staleness. **S**.

- **PKGCACHE-3**: Background refresh job. Spawns once per
  session if stale, gated on offline-mode + connectivity +
  user-not-disabled. Runs the batched query for each detected
  package manager. Writes the result atomically (write to
  temp, rename). **M**.

- **PKGCACHE-4**: Suggestion engine using the layered index.
  Given a missing-binary name from NIXFIX-1, look up:
  curated first (instant), then local index (instant if
  loaded). Emit one or more install commands. **S**.

**Arc PKGUI: suggestion button UI.**

Inline below the failed command, before the next prompt.
Pill-shaped, theme-aware, tappable. Each pill shows one
suggestion. Multiple package managers → multiple pills side
by side.

Visual: similar to the reading-overlay pills from v0.1.248
(softly rounded, ~30px tall, theme-aware border). Suggestion-
accent color distinct from selection-accent (avoid confusion).
Probably `th.ac` with reduced opacity for the non-primary
suggestions.

Pill content: `[nix shell nixpkgs#nodejs]`, `[apt install nodejs]`,
`[python -m venv .venv]`, etc.

Tap behavior:
- If safe (no sudo, no destructive flags): insert at prompt
  for user to press Enter
- If sudo required: insert, prompt for explicit confirm before
  Enter
- If destructive (rm, force flags): never auto-insert; show
  warning

**PKGUI sub-ships:**

- **PKGUI-1**: Pill renderer in terminal pane. Anchors to
  the prompt position. Hides on alt-screen (TUI active).
  Hides on next prompt (suggestion is for THIS failure
  only). **M**.

- **PKGUI-2**: Tap-to-insert wiring. PTY-write the
  suggestion at the cursor. **S**.

- **PKGUI-3**: Confirm-on-sudo flow. **S**.

- **PKGUI-4**: Suggestion dismissal/multiple variants.
  User can dismiss the suggestion. If multiple managers
  apply, user can pick. **S**.

**Arc OFFLINE: offline mode toggle.**

Single setting, persists via save_settings infrastructure.
Quick-settings toggle in the drawer.

**What offline mode disables:**

- Claude API calls (chat module — already conditional on
  presence of API key; offline mode forces the "not
  available" path)
- Ollama localhost requests (yes — localhost still counts
  as network for the toggle's purpose; user opting into
  offline wants ALL LLM features off)
- General `web_fetch` / `ureq::get` for navigation
- Package index refresh (PKGCACHE-3)
- Future cloud-voice integration

**What offline mode does NOT disable:**

- Markdown rendering (local — applies to manpages and
  cached HTML)
- Manpage navigation `web2qml://man/...`
- File:// navigation
- Reading-overlay + phoneme database (local)
- Terminal pane (local)
- Whisper voice recognition (local)
- Local file system operations
- Archive viewing (local)
- table_nlp / shell_nlp lexicon (local)
- Package suggestion using LOCAL bundled curated subset
  AND last-cached system index

**Subtle: offline-mode-aware archive fallback.** When user
navigates to a URL in offline mode and we have an archived
version of that URL, serve the archive instead of failing.
The archive infrastructure already exists; this is wiring it
as the offline fallback path.

**Composition with other features:**

- `llm_available()` returns false when offline_mode is true,
  regardless of other state. SNLP-6 (Python NLP) becomes
  unavailable. Recovery message: "Python NLP requires LLM;
  offline mode is enabled."
- PKGCACHE-3 refresh job is gated on `!offline_mode`. Curated
  subset still works.
- PKGUI suggestions can still appear (using local data) but
  the "fetch the latest version info" enhancement is
  disabled.

**OFFLINE sub-ships:**

- **OFFLINE-1**: Add `offline_mode: bool` qt_property +
  setter. Persist in settings.conf. Default false (unless we
  detect no network, then prompt? — probably no, just
  default false and let user toggle). **S**.

- **OFFLINE-2**: Wire the gate into all network-touching
  paths. Inventory: chat module, web_fetch in browse nav,
  Ollama health probe, future package refresh. Each gets a
  `if backend.offline_mode { return early }` check. **M**.

- **OFFLINE-3**: Quick-settings UI. Toggle in the drawer.
  Visible badge somewhere when active (so user remembers
  they enabled it). Probably a small "offline" indicator
  near the URL bar or in the drawer. **S**.

- **OFFLINE-4**: Archive fallback for navigation. When
  offline + URL has been archived, serve archive instead
  of failed nav. **M**.

- **OFFLINE-5**: User-facing affordances. When user
  navigates to a non-archived URL in offline mode, the
  empty-state message should explain: "Offline mode is on.
  Disable it to fetch, or navigate to an archived page."
  Probably reuse the existing offline-empty-state
  infrastructure if it exists. **S**.

**Sequencing across all three arcs:**

This intersects with NIXFIX. Probably best order:

1. **OFFLINE-1 + OFFLINE-2** — the toggle infrastructure
   first, so other features can gate on it from the start.
2. **PKGCACHE-1** — bundled curated subset (build-time data,
   no runtime refresh needed; safe).
3. **NIXFIX-1 + NIXFIX-2 + PKGCACHE-4** — failure detection
   + package manager detection + curated-only suggestion
   engine. First end-to-end "command fails → suggestion
   appears" flow.
4. **PKGUI-1 + PKGUI-2** — suggestion pill UI.
5. **PKGCACHE-2 + PKGCACHE-3** — system-local index loader
   and refresh job. Expands suggestion coverage beyond
   curated.
6. **PKGUI-3 + PKGUI-4** — sudo confirm + multi-variant.
7. **OFFLINE-3 + OFFLINE-4 + OFFLINE-5** — UI for offline
   mode + archive fallback + empty-state messaging.

Steps 1-2 are independent and small. Step 3 is the
end-to-end win (first real "command recovery" feature).
Steps 4-7 build on that.

**Total scope:** ~12-15 sub-ships, 3-5 months if done
seriously. Same long-horizon shape as phoneme alignment.

**Defer until** hardware validation of the v0.1.241-249
cascade is done. Then PKGCACHE-1 is a good opener because
it's self-contained (script + data file + loader, no
network dependencies) and lets us demonstrate the suggestion
flow even before OFFLINE/PKGUI/NIXFIX land.

### v0.1.251 hardware test feedback (Allison session 2)

User tested v0.1.251 with Allison present again. Multiple new
findings.

#### Confirmed working

- Speaker color separation (v0.1.251) landed cleanly. Allison's
  icon rendered yellow on this session, visually distinct from
  user. No `[speakers] recolored` lines in log — meaning the
  migration ran on a prior session and saved successfully,
  duplicate-color check passed on this load.

#### New bugs filed

- [x] **S — Teleprompter font too small for shared-screen
  reading.** **Landed v0.1.257.** Default font went from 16 →
  28; new ±A cycle button (20/28/36/44 presets) in the overlay
  bottom row; persists to `~/.config/web2qml/reading_font_px`.
  Reading-state height scales with font so larger sizes get
  more vertical context.

- [x] **M — Speaker mis-classification via pitch-hint
  persistence across speakers.** **Landed v0.1.258.** Sharp-
  difference detection in `phonetic::top_matches_with_hint`:
  if hint pitch and query pitch differ by >1.3× ratio, reject
  the hint and use query-derived pitch instead. Log line
  `[phonetic] rejecting stale pitch hint:` shows when the
  guard fires. Conservative threshold (1.3×) safely above
  intra-speaker variation, below the 1.43× cross-speaker
  ratio observed in the hardware bug.

- [x] **S-M — Duplicate speaker entries created when names
  differ.** **Landed v0.1.259** (option 1 — voice-profile dedup
  on register). Registration popup now calls
  `speaker_propose_dedup` BEFORE committing; if existing speaker
  matches above 0.85 similarity, surfaces a confirmation dialog
  with "Rename to <new>" vs "Register as new" options. Picking
  rename calls `speaker_rename` (no duplicate created). The
  threshold 0.85 (vs the 0.65 identify threshold) biases toward
  false-negatives — similar-but-distinct voices don't trigger
  spurious merge prompts.

  Option 2 (manual merge UI in settings) — **landed v0.1.262.**
  ⚙ button in the speaker dots row opens a popup with all
  registered speakers; two-tap selection (source → target);
  merge button blends profiles and removes source. Target's
  allowed/deep_muted flags WIN (security: blocked source can't
  unblock by merging into allowed target).

- [x] **S — Natural-language speaker disambiguation.** **Landed
  v0.1.260.** New module `src/speaker_claim.rs` parses
  "Hey/I'm/This is <name> ..." patterns from Whisper output,
  fuzzy-matches against registry (handles Whisper errors like
  "castle"→"Kessler" via Levenshtein), feeds into the voice
  identify path. Security boundary preserved: claim never
  overrides confident acoustic identification; only helps when
  acoustic is uncertain. Log line `[speakers] using
  claim-based identity ... (acoustic uncertain)` when the
  claim provides identification.

- [x] **observed — pitch hint bin 18 has no indexed
  fingerprints.** **Landed v0.1.261** (log throttle).
  Per-bin once-only logging via OnceLock + HashSet. Root cause
  largely addressed by v0.1.258 (pitch-hint rejection); this
  ship just cleans up the residual log noise for edge cases.

#### From user, deferred per their direction

User said:
> "the rollback confusion UI stuff needs to land before I can
> try this type of stuff... same with the database git backend
> going back and realigning data"

i.e. pause multi-user speaker testing until rollback
infrastructure exists. Acted on by starting the GITLOG arc
(v0.1.252 ships GITLOG-1 audit log foundation).

### v0.1.249 hardware test feedback (filed before fixing)

User ran the v0.1.249 build with the test sandbox. Lots of
real signal. Filed grouped by category.

#### Confirmed working (no action)

- Per-content-type WPM multipliers (PA1 v0.1.242) — log shows
  `[paragraph] selected p_10 (table, 58 words)`, prose, etc.
  Content types correctly classified.
- Phoneme database lookup (PA3 v0.1.246) — log shows
  `phonemes: 38 words from welcome` and `64 words from
  onboarding` for manpages; correct fallback `not bundled`
  for web content paragraphs.
- Whisper hallucination defense — log shows
  `[whisper] trimmed hallucination loop (6x repetition of 'run
  cargo test,')` correctly catching loops.
- Helix scrolling no longer prints `<5`/`<6` characters
  (touch-drag bug from v0.1.245 filing). User confirmed:
  "helix scrolling works now."
- Voice-driven file resolution: log shows
  `[entity] resolve_blocks produced 2 compositions` with
  appropriate scoring (1.00 for filename-shape + content
  match, 0.90 for tree-only). The "open testing guide" →
  `$EDITOR TESTING-GUIDE.md` flow worked end-to-end.

#### Bugs filed for fix

- [x] **S — v0.1.249 URL bar at bottom collides with settings
  & voice buttons in landscape.** **Partly landed v0.1.279.**
  The orientation-aware default (URL bar at TOP in landscape,
  BOTTOM in portrait) shipped — implemented via dynamic-parent
  reparenting between two mount slots in the main ColumnLayout,
  auto-detected via `width > height`. The "URL bar position
  setting (auto/top/bottom)" preference and the wider
  bottom-overlay audit are still open — refile separately if
  hardware test reveals remaining landscape collisions in
  the bottom area.

- [x] ~~**S — Two speakers both red dots; need color separation.**~~
  **Landed v0.1.251.** Root cause: v0.1.211's
  `color_from_profile` clamped pitch_hz to 80-350Hz (human
  voice fundamental range), but our ZCR-based pitch detector
  produces values 400-1500Hz on real speech (because it counts
  zero crossings rather than detecting f0). Both Kessler (~680Hz)
  and Allison (~975Hz) clamped to the same endpoint hue.
  
  Fix: widened pitch range to 80-1500Hz + added ±60° hue
  modulation from spectral centroid. Combined effect: two
  speakers with distinct profiles get distinct colors. Migration
  recomputes colors retroactively for existing registries with
  identical-color speakers. Three new regression tests.

- [x] **M — Guest-speaker first-experience is broken; the
  onboarding moment is exactly when we mute them silently.**
  **Landed v0.1.263 (option 1, enrollment prompt).** MUTED
  sentinel handler in QML now opens `_mutedEnrollPopup`
  explaining the situation with two buttons: "Allow voice
  commands" (calls `speaker_set_allowed(name, true)`) and
  "Keep muted" (dismiss). Single-tap path for the device owner
  to enable a known-but-muted speaker on the spot. Options 2
  (guest mode) and 3 (tiered speakers) remain queued but the
  immediate Allison-blocker is resolved.
  
  Context: user invited Allison (non-technical tester, doesn't
  work in tech, doesn't live in terminal — the ACTUAL target
  audience) to try the app while v0.1.249 was running. The
  speaker-gating code identified her correctly with high
  confidence (sim=0.941 on best, 0.870, 0.882) but discarded
  most of her utterances with `[speakers] "Allison" not
  allowed — discarding utterance` because she's not on the
  whitelist.
  
  Her log-visible attempts:
  - "cat the testing guide" → discarded
  - "cat testing guide" → discarded
  - "open testing guide" → eventually worked (one squeaked
    through because her speaker_match_score was 0.569, below
    threshold)
  - "yes" (trying to confirm a candidate) → discarded
  - "click helix" → went through but attributed to user
  - "close helix" → went through but attributed to user
  
  UX flow she experienced: speak, popup closes, brief toast
  "Ignored (muted): Allison", nothing actionable. The
  suggestion candidates that Whisper successfully transcribed
  were never shown to her. From her perspective: try the
  app → nothing happens → try again → nothing happens. Worst
  possible onboarding for a new tester.
  
  The existing MUTED handling (qml/main.qml line 10470-10485
  from v0.1.119) shows a toast and closes the popup. That's
  technically informative but it's a dead end — no path for
  the new speaker to actually USE the app.
  
  **Three architectural options, ranked by ambition:**
  
  1. **Enrollment prompt on first muted utterance.**
     When MUTED sentinel arrives, instead of just toast +
     close, show a popup: "I heard '<their words>' but you're
     not registered to issue commands. Would you like to
     enroll <name>?" with tap-yes / tap-no. Yes → enroll
     them as a permitted speaker. Cheapest fix, addresses
     the immediate Allison problem. **S-M.**
  
  2. **Guest mode with confirmation gate.**
     Identified speakers who AREN'T on the whitelist still
     get their utterance parsed for intent. The suggested
     candidates are SHOWN but require explicit tap to
     execute (no auto-execute). For destructive commands
     (rm, sudo, etc.) require a second confirmation. This
     gives Allison the experience of being heard and seeing
     what the system understood, while preserving the
     security model. **M.**
  
  3. **Tiered speakers (owner / guest / blocked).**
     Owner: existing whitelist, all commands allowed.
     Guest: limited command palette (browse, read-aloud,
     terminal scrollback, but not write / sudo / network).
     Blocked: discarded silently (current behavior, kept
     as opt-in for known nuisance speakers).
     Most flexible, most code. **L.**
  
  Recommendation: ship option 1 first because it solves the
  immediate problem (Allison can't demo). Then if owner+guest
  becomes a common pattern, layer option 2 on top.
  
  Touchpoints:
  - `src/main.rs:6716-6720` (where MUTED sentinel originates)
  - `src/speakers.rs` (allowed_speakers list)
  - `qml/main.qml:10475-10485` (where MUTED sentinel is
    consumed and toast shown)
  
  Connects to: speaker-color fix (Allison needs a distinct
  color too), and the "tap-and-hold for refinement" idea the
  user said to wait on. The first-utterance enrollment
  prompt is a different mechanic from refinement — it's
  pre-enrollment, not corrective.

- [ ] **observed → understood — "Allison" auto-registration
  was a guest tester, not a bug.** User clarified:
  > "I was trying to show Allison the app and make her speak
  > she is a good tester because she does not work in tech
  > and does not live in the terminal"
  
  Filed for future log-reading: when speaker count grows
  unexpectedly, it's often a deliberate demo, not a
  classifier bug. The classifier did correctly identify her
  as a distinct speaker (high embedding confidence) — the
  problem is what happens NEXT (filing above).
  
  Side note on the threshold: her scores ranged 0.500 →
  0.941 across utterances. Lots of variance. Some scored
  ABOVE the user (suggesting her embedding is more
  distinctive than expected), others scored as low as 0.500
  (basically uncertain). The variance is probably due to
  chunk length, background noise, and pitch-hint mismatch
  (her pitch ~975Hz hit the "pitch hint miss for bin 19"
  fallback path repeatedly — see the `[phonetic] pitch
  index miss for bin 19, falling back to full scan` log
  spam). Worth investigating once we have more multi-speaker
  data.

### Multi-user shell architecture (filed v0.1.250 conversation)

User direction after the Allison guest-tester episode:
> "your three step plan with permission systems is really
> nice... you could gate access to folders like user
> permissions and register users and plug into their shell
> using seat and logind since we only have one terminal tab
> and we could get collaborative shells going where we can
> have up to one shell per user and they can be put on the
> screen and passed mouse and keyboard back and forth or we
> can take our inline command blocks idea from a while ago
> and start weaving the terminal via another git commit log
> with commit msg or metadata that points to the command
> block we want so we can have rollback and instant loading
> and use our animation system to wiggle in starwars style
> but rollback halfway on utterances as a visual"

Four braided ideas, separated:

**(a) OS-level permissions for speakers.** Each registered
speaker becomes a real Linux user, with filesystem ACLs,
group memberships, and sudo rules acting as the permission
gate.

Honest assessment:
- **Pro**: battle-tested permission model, no reinventing.
- **Con**: requires sudo at speaker-enrollment time. Major
  privilege escalation for an app currently running fully
  user-mode.
- **Con**: voice mis-classification could attribute commands
  to wrong real user, with real filesystem consequences. The
  Allison log shows speaker_match scores 0.500-0.941 across
  consecutive utterances — too much variance to safely gate
  destructive commands on.
- **Con**: only works on Linux. macOS doesn't have the same
  model.
- **Con**: forces user to think in Unix-user terms when they
  were thinking in "people in the room" terms.

**Distro-respect principle (user direction, anchor for this
arc):**
> "we just have to respect the distro ways of doing their
> user stuff... never break it"

This rules out the "we just shell out to `useradd`" shortcut.
Each distro has its own conventions:
- **Fedora/RHEL**: `useradd`, sssd/freeipa-aware,
  SELinux contexts on home dirs, polkit for desktop
  privilege, /etc/login.defs.
- **Debian/Ubuntu**: `adduser` (NOT `useradd`!) wrapping
  with different defaults, AppArmor profiles, sudo group,
  /etc/skel customizations.
- **Arch**: `useradd` only, no `adduser` wrapper, minimal
  defaults, expects explicit configuration.
- **NixOS**: users declared in `configuration.nix` and
  applied via `nixos-rebuild`. Calling `useradd` directly
  creates a user that disappears on next rebuild.
- **Alpine**: BusyBox `adduser` with different flags than
  Debian's.
- **Immutable (Silverblue, MicroOS, bootc)**: read-only
  rootfs, user management goes through different paths
  (e.g. `rpm-ostree` or layered systemd-sysusers).

If we ever DO add real-user enrollment, it has to:
1. Detect the distro (probably `/etc/os-release` + presence
   of specific tools like `nix-store`, `rpm-ostree`,
   `apt-get`, `apk`).
2. Use the distro's native path (`useradd` vs `adduser` vs
   editing configuration.nix vs whatever the immutable
   distro wants).
3. Respect distro-specific security frameworks (SELinux
   contexts on Fedora, AppArmor on Ubuntu, no SELinux/
   AppArmor on Arch).
4. NEVER bypass the distro's convention "because we know
   better." Even if it means a feature isn't available on
   some distro.

The detection + per-distro implementation matrix is
substantial. For NixOS specifically there's no clean
runtime-add-user path that's reproducible — declarative
config is the only respectful approach.

More conservative middle ground (recommended): keep
speaker records in our app (speakers.rs already does
this), DON'T create real OS users at all. App-level
identity (Allison's fingerprint) maps to "guest" tier
in our model, NOT to a real Linux user. When Allison
says `cat ~/foo` we still run it as the logged-in OS
user but with our app's guest-tier restrictions applied
first.

This sidesteps the distro-respect matrix entirely because
we're never touching distro user management. The app-level
tier model is portable across all distros and macOS.

**Shim-with-attribution-log refinement (user direction):**
> "we can shim: pass fake users through to valid system
> users for sudo when we need with some echo logging or
> something like that"

The "shim" pattern: when a speaker's intent requires sudo
or any privileged operation, we DON'T add the speaker as a
real Linux user. We execute as the logged-in OS user
(whoever owns the running app process), using THEIR existing
sudo creds (with their explicit consent at the moment of
execution), and we write a structured audit-log entry that
attributes the action to the speaker.

Important framing note: this is **attribution**, not
**impersonation**. We're tracking WHO ASKED for the action
while the action RUNS as the system's actual user. App-level
permission tier (Allison's guest tier) governs WHETHER the
attempt is allowed; the audit log records the attempt
regardless. We never let the user form a mental model where
Allison is somehow a separate OS user with isolated files.
That model is wrong and would be a security footgun.

**Audit log shape (not "echo logging" — proper structured
log):**

- Path: follow XDG_STATE_HOME convention, e.g.
  `~/.local/state/web2qml/audit.log` (with fallback to
  `~/.config/web2qml/audit.log` if STATE_HOME not set)
- Append-only writes, fsync on each entry so a crash
  doesn't lose recent attributions
- One line per event, structured (JSON or key=value)
- Fields:
  - `ts`: ISO 8601 UTC timestamp
  - `speaker_id`: stable hash identifier from speakers.rs
  - `speaker_name`: human label (e.g. "Allison")
  - `speaker_confidence`: float, 0..1, from the embedding
    similarity at the moment of attribution
  - `attestation_method`: `voice` | `tap_confirm` | `typed`
    (some intents may be voice-attested only, others
    require a tap to execute, others are typed)
  - `intent_kind`: term_cmd / nav / read_aloud / etc.
  - `intent_param`: the resolved command or URL
  - `cwd`: working directory at execution time
  - `sudo_used`: bool — did this invoke sudo
  - `exit_code`: int, populated after the command returns
  - `actual_os_user`: the OS user the process ran as
    (almost always the logged-in user; documented for
    completeness)
- Rotation: rotate at N MB (probably 10MB) or weekly;
  compress old logs. Don't delete automatically — audit
  trails should outlive memory.

The audit log is the actual security artifact, not the
UI. The UI can lie (say "Allison did this" when classifier
was wrong); the audit log records "Allison ATTRIBUTED with
confidence 0.55", which is honest about the uncertainty.

**Failure modes to think through:**

1. **Destructive commands with high stakes**: Allison says
   "sudo rm -rf X". App-level tier check MUST happen
   before sudo is even invoked. Audit log is the second
   layer, not the only layer. Guest tier should deny
   destructive sudo flat. Owner tier can allow but with
   confirm.
   
2. **Mis-classification consequences**: Speaker confidence
   0.55 means "we're guessing." For low-confidence
   attributions, require explicit tap-confirm before
   executing destructive or sudo commands. Tap is a
   different modality (keyboard or screen), serves as
   human-in-the-loop check. The audit log records
   `attestation_method=tap_confirm` so post-hoc audit
   shows the user explicitly intervened.

3. **Audit log tampering**: append-only writes help but
   don't fully prevent a malicious local user from
   editing the log. For high-security cases we'd want
   append-only filesystem attributes (chattr +a on Linux),
   syslog forwarding, or hash-chained entries. These are
   significant additional complexity and probably
   overkill for v1; file as future hardening if we ever
   see real adversarial use.

**Implementation sketch (sub-ships):**

- **ATTR-1**: structured audit log infrastructure. New file
  `src/audit_log.rs` with `pub fn record(event: AuditEvent)`.
  Lock-protected append-only writer; rotates at 10MB.
  Fsync on each write. ~150-300 lines. **S-M.**

- **ATTR-2**: wire into intent execution. Every executeIntent
  call from QML now goes through the audit log first, with
  speaker_id and confidence from the voice path attached.
  Pure-typed intents log `attestation_method=typed` with
  empty speaker_id. ~50-100 lines plus call site updates.
  **S.**

- **ATTR-3**: tier-based pre-execute gating. Guest tier
  has a denylist of intents (sudo, rm with destructive flags,
  chmod outside home, etc.). Mid-confidence attributions
  on destructive commands route to tap-confirm path
  regardless of tier. ~200-400 lines. **M.**

- **ATTR-4**: audit log viewer. Voice command "show me
  the audit log" or settings tab "Audit log" → in-app
  pane that paginates the log, filters by speaker, by
  date, by intent kind. ~M. Probably deferred until
  ATTR-1+2+3 prove themselves.

  **Partially landed v0.1.270.** Basic viewer at
  `web2qml://audit` renders the last 200 entries as
  markdown via `manpages::to_qml`. Most-recent-first,
  shows timestamp, speaker + confidence, command, cwd,
  exit code, duration, attestation method, snapshot
  indicator. Filtering by speaker/date/kind and voice-
  intent routing ("show audit log") remain queued.

This is the right architecture for "voice-attributed
commands on a shared device" and it generalizes well: same
infrastructure works for in-room collab (option c), for
future remote attribution if we ever support it, and for
the GITLOG arc (option d) where commit messages would
include speaker attribution from the audit log.

**Recommendation update**: don't just "skip OS-user
integration." Build ATTR-1 + ATTR-2 as the foundation
for tiered permissions, and keep the door open for
mapping app-level Allison to a real OS user later (per
distro-respect: only if the user creates the OS user
themselves via their distro's normal tools).

The shim approach IS the long-term answer, not a temporary
expedient. Attribution-without-impersonation is the
correct security model for a voice-first device. Real
OS-user separation is for cases where physical access is
also separated (different keyboards / different sessions),
which is not our model.

**(b) seatd/logind integration.** systemd-logind tracks
user sessions, seat assignments, virtual terminals. Idea
is to spawn each speaker's commands inside their own
logind session.

Honest assessment: logind's model is "this physical input
device belongs to this seat belongs to this user session."
We don't have multiple physical input devices. We have one
keyboard + N voice inputs distinguished by acoustic
signature. There's no clean place for "voice speaker" in
logind's vocabulary; we'd be bending it to mean something
it doesn't.

The actual primitive logind provides — "run this process
as user X" — we can get from `sudo -u allison <cmd>` or
direct PAM auth without involving logind at all.

Distro-respect also applies here: logind is systemd-only.
Alpine and Void and some lightweight distros don't run
systemd. Building on logind tacitly excludes those users.

Recommendation: skip this. Complexity tarpit for no
unique value, and adds a systemd dependency where we
don't currently have one.

**(c) Collaborative tiled shells.** Single terminal tab
becomes a layout of N shells, one per registered speaker.
Mouse focus indicates active keyboard target; voice routes
by speaker identity to the correct shell.

Honest assessment: this one is genuinely interesting and
moderately scoped.
- **Pro**: enables in-room pairing (two devs voice-first
  on one device, watching each other's shells).
- **Pro**: enforces shared attention by being single-tab.
- **Pro**: existing PTY+grid infrastructure largely works
  per-shell; we'd need N PTYs and a tiling renderer.
- **Pro**: voice-routing is trivial if speakers.rs has the
  PTY-binding data.
- **Con**: only helps in-room collab. Remote requires
  session sharing (mosh, tmate) which is a different scope.
- **Con**: small screens (phone) probably can't tile N
  shells legibly. Mobile likely stays single-shell.
- **Con**: focus management for keyboard is its own
  subsystem (which shell does typing go to? click vs
  voice for switch?).
  
Estimated scope: 3-4 ships.
- COLLAB-1: per-speaker PTY allocation (extend pty_term
  to hold a vec of PTY+grid instead of one)
- COLLAB-2: tiling renderer for the terminal pane (2-way
  vsplit on desktop, hsplit when wider, stack on mobile)
- COLLAB-3: focus management (active-shell highlight,
  keyboard routing, voice routing)
- COLLAB-4: shell switcher UI (tap a shell to focus, voice
  command "switch to Allison's shell" to focus)

**M-L total.** Plausible after the simpler enrollment
work lands and we have real-world data on whether
in-person collab is the actual use case.

**(d) Git-commit-log as terminal scrollback.** Most
ambitious. Each command + its output + metadata becomes a
git commit. Scrollback is a navigable timeline. Rollback
= `git checkout`. Visual: command blocks as discrete
shifting objects; rollback animates as Star Wars wipe /
3D rotation; "rollback halfway on utterance" partially
reverses the animation, visually matching voice
tentativeness.

Genuinely interesting design ideas inside this:
- Versioned command history with cryptographic integrity
- Branching: fork from any commit, try alternative paths
- Replayable demos (share git ref, recipient gets the
  full chain)
- "Half-rolled-back" animation as a visual analog of voice
  uncertainty — committed only on utterance completion,
  reverses on cancel. This is a beautiful UX moment.

Honest assessment of what it can and can't do:
- **Can rollback**: filesystem state under git's tracking,
  cwd, env diff (if captured in metadata).
- **CANNOT rollback**: network requests sent, emails fired,
  database mutations, GitHub PRs created, any side effect
  outside git's awareness. `git checkout` doesn't unsend
  email.
- **Performance**: 100s of commits/hour is plausible for an
  interactive session. Git handles it but objects pile up
  fast. Need GC discipline.
- **Cross-shell semantics**: rolling back to commit N means
  what for cwd? env? open FDs? File descriptors can't be
  restored at all.
  
The "rollback halfway on utterance" visual has clean
semantics only when the command is git-trackable. For
side-effect commands (curl, sudo, db client), the visual
would be misleading — we'd animate a rollback that
doesn't actually undo the side effect.

Mitigation: command classifier flags "git-rollbackable"
vs "side-effecting" commands. Only show the rollback
visual on the former. Side-effecting commands get a
different animation (lock-in, no rollback) and explicit
"can't undo" affordance.

Estimated scope: 5-7 ships.
- [x] **GITLOG-1**: hash-chained append-only audit log of
  command-blocks (cwd, cmd, output, exit code; speaker
  fields present in schema but not yet wired from voice
  path). **Landed v0.1.252.** Decided NOT to use real git
  for this layer; chose self-contained line-format with
  SHA-256 chain to avoid (a) spawn cost per command and
  (b) hard dep on git binary. Real git becomes worthwhile
  in GITLOG-2 when tree storage is justified by filesystem
  snapshotting.
- [x] **GITLOG-1b** (S): voice-attestation wiring. **Landed
  v0.1.253.** The Entry struct supports speaker_id,
  speaker_name, speaker_confidence, attestation fields; voice
  identify path now sets pending attribution that the next
  block-close consumes. Single-shot + 30s staleness timeout
  prevent mis-attribution.
- [x] **GITLOG-2** (M): filesystem snapshotting. **Landed
  v0.1.254.** Decided to keep our own content-addressed
  format rather than migrate to real git for v1 — same
  reasoning as GITLOG-1 (no spawn cost, no binary dep, no
  lock contention with user's own git). Snapshot store at
  `~/.local/state/web2qml/snapshots/{blobs,trees}/...`.
  Bounded by MAX_FILES_PER_SNAPSHOT=2000 to prevent
  pathological dirs from killing per-command latency.
  `restore_tree(hash, target)` primitive exists for GITLOG-3
  to call. Real git migration deferred; will revisit when
  GITLOG-6 (replay-from-ref) actually needs `git push`.
- [x] **GITLOG-3** (M): rollback command + voice trigger + visual
  animation. **Landed v0.1.255.** Per-block ↩ button in
  blocksDrawer (visible only on blocks with a tree snapshot),
  voice intent "undo"/"rollback"/"scratch that"/etc. routes to
  most-recent-rollbackable. Full wipe animation on the affected
  block. Conservative v1: additive restore (no deletes),
  restore-to-original-cwd (not current cwd), no confirmation
  dialog. Half-wipe-on-utterance variant deferred until voice
  pipeline supports partial-utterance detection.
- [x] **GITLOG-4** (S-M): command classifier (rollbackable vs
  side-effect). **Landed v0.1.256.** New module
  `src/cmd_classifier.rs` classifies each command as ReadOnly,
  Mutating, or Unknown. snapshot_dir only called for the
  latter two — read-only commands like ls/cat/pwd/grep/git
  status no longer trigger snapshots. Conservative bias:
  unknown commands snapshot (safe default).
- [x] **GITLOG-5** (M): branching / fork-history UX.
  **Landed v0.1.274.** Parent-pointer tracking on each
  CommandBlock. After a rollback, the next command becomes a
  SIBLING of the rolled-back tip (not its successor) via
  `set_parent_after_rollback`. New viewer at
  `web2qml://branches` renders the tree as nested-bullet
  markdown. Voice intent "show branches" / "show command
  tree" navigates there. Persistence of parent pointers in
  the audit log file (so branches survive restart) deferred.
  Tree navigation (tap-to-switch between branches) filed as
  GITLOG-5b.
- **GITLOG-6** (M): replay-from-ref (share command chains).
  Likely needs real git for `git push`.
- **GITLOG-7** (S, deferred): integrity hardening — chattr
  +a, syslog forwarding, audit-log verification command.

**L total.** This is its own multi-month arc. The "rollback
half on utterance" visual is the strongest emotional pull
but probably comes near the end of the arc, not the start.

**Recommended sequencing (honest):**

This is a 6-12 month redesign of the shell paradigm if
pursued fully. The right opener is NOT to file these and
forget them; it's to ship the small enrollment prompt FIRST
to prove we can solve Allison's actual problem on her next
visit. Then:

1. **Ship soon (S)**: speaker color separation + first-mute
   enrollment prompt (already filed). Closes the Allison
   gap. Validates the demo-tester flow.
2. **Then (M)**: tiered permissions in-app (option 2 from
   the guest-speaker filing). Builds the permission model
   we'd want to USE for COLLAB and GITLOG features.
3. **Then (M-L)**: COLLAB-1 + COLLAB-2 (multi-PTY +
   tiling). Most concrete user-visible value of this whole
   thread.
4. **Eventually (L)**: GITLOG arc. Big design risk; the
   rollback semantics need a lot of careful thought. Don't
   start until COLLAB has shipped and we know whether
   multi-user is the actual usage pattern.
5. **Probably never**: OS-level permission integration
   (option a), seatd/logind (option b). Cheaper to keep
   permissions in-app.

If we get to step 3 and the multi-shell pattern isn't
showing real value, prune the GITLOG arc and refocus.

**Defer for now.** Speaker color + enrollment prompt is the
next concrete ship. Everything in this filing depends on
that landing first.

- [x] **S — Folder picker: no "up one folder" button.**
  **Landed v0.1.263.** Header up-arrow already existed; user
  wanted an in-list `..` entry per ranger / mc / DOS
  conventions. Single-pane file list now prepends `..` row
  (italic, up-arrow icon) at the top of every directory
  listing except filesystem root.

- [x] **S — Voice popup initial waiting state has bottom
  overflow.** **Landed v0.1.263.** Bumped popup height
  padding 40 → 56 to absorb the implicitHeight undercount
  from embedded Layouts using preferredHeight. Excess
  becomes harmless whitespace when content fits exactly.

- [x] **S — Settings icons lost antialiasing.** **Landed v0.1.269.**
  Root cause: ToolBtn's icon Text element had no explicit
  `renderType`, defaulting to `Text.NativeRendering` which can
  fall back to monochrome glyph bitmaps for Unicode symbol
  glyphs at certain sizes. Fix: `renderType: Text.QtRendering`
  on the icon Text. Affects all ToolBtn-based buttons (back,
  forward, hamburger, settings, lightning, etc.). If hardware
  test shows other icons (URL bar, popup buttons) still hard-
  edged, those use direct Rectangle+Text and need similar
  treatment — file with screenshot.

- [~] **M — Helix startup latency huge vs konsole instant.** DIAGNOSTIC PREP v0.1.434: profile-gated timing added (profile_on, altscreen:enter cost, pty:parse burst timing). AWAITING a WEB2QML_PROFILE=1 hardware run to localize the cost before the fix.
  User feedback:
  > "helix scrolling works now, but still takes very long
  > to open helix vs instant on konsole... that's a huge
  > letdown that we should look into further (file it)"
  
  Real cost comparison should be measured: time-from-keystroke
  to first-frame-rendered in our pty_term vs in konsole.
  Probably we're doing too much work synchronously on alt-screen
  enter (full grid resize? heavy redraw?).
  
  Diagnosis:
  - Add `eprintln!` timestamps around alt-screen-enter event
    handling in pty_term.rs
  - Profile with WEB2QML_PROFILE=1 (already supported, log
    shows it's set), look for frame-time spikes near
    `altscreen:enter`
  - Compare grid-clear vs grid-resize costs
  - Compare what konsole does — probably keeps grid memory
    allocated and just swaps screen buffers, while we may
    be reallocating
  
  Connects to TP4 work (Konsole-baseline profiling, already
  filed). **M.**

- [x] **S — Helix tap-to-reposition cursor still broken.**
  **Landed v0.1.266.** New `encode_tap_event` in pty_term.rs
  encodes a press+release pair for SGR/urxvt/X10 mouse
  protocols. New `term_send_tap_report` qt_method dispatches
  it. Terminal MouseArea's `onClicked` now checks
  `term_mouse_reporting()` first and routes click events to the
  TUI when active. Pre-existing focus/paste fallback preserved
  for non-mouse-tracking programs. Drag-to-select inside TUIs
  (button1 + motion) filed as separate S follow-up.

- [ ] **M — Local valid markdown tables not rendered.** User
  feedback:
  > "my local markdown tables that are valid but not formatted
  > are not being rendered"
  
  "Valid but not formatted" — probably tables without column
  alignment (`---` row without spaces, or pipe-only without
  proper spacing). Our markdown renderer might be too strict.
  
  v0.1.452: relaxed BOTH separator checks to accept en/em-dash
  (smart-dash autocorrect artifacts) — a strong hypothesis for
  this report, found by stress-testing. NOT yet confirmed against
  the user's file; if still broken, suspect pipe-less single-col
  tables or inline pipe-escapes. Original note follows.

  Fix: relax the table grammar to accept unformatted-but-valid
  tables. Compare against the CommonMark + GFM table spec
  (the lenient one). Look for test cases in our markdown
  parser. **M.**

  Investigated v0.1.264 session: the parser (`parse_markdown_table`
  in table_ops.rs + `looks_like_table_separator` in manpages.rs)
  IS lenient — handles missing edge pipes, tight spacing,
  alignment colons. Without the user's concrete failing example
  it's hard to reproduce. The original test cases pass: bare-pipe
  tables (`a|b\n-|-\n1|2`), edge-pipe-only-on-header,
  alignment-separators, etc. Possibly the failure is around
  inline content within cells (asterisks, backticks, links) or
  pipe-escape edge cases. Need to ask user for the actual file
  content next session.

  v0.1.882: added regression tests for every named hypothesis —
  single-column tables, escaped pipes (`\|` → literal), and inline
  markup in cells (`*`/`` ` ``/links kept verbatim at parse, styled
  at render). All pass. The parser is confirmed robust on these
  axes, so this report can't be reproduced without the user's
  actual file; closing the speculative angles, awaiting the file.

- [x] **L — Raw `.md` URL serves raw text, not rendered
  markdown.** **Landed v0.1.265.** Detection branch in
  `load_page` triggers when URL path ends in `.md`/`.markdown`
  AND content doesn't lead with HTML markers. Routes through
  `manpages::to_qml`. Cached as `NativeQml`. Bonus user
  direction about zettelkasten/critical-thinking integration
  remains a separate filing.
  
  Bonus user direction (also worth scoping):
  > "I guess I'm now scoping out how this could link to
  > zettelkasten later... also can't read anything on this
  > page and would be good for reading code out loud as
  > corpus lexicon/thinking-question-voice aid training and
  > as critical thinking tool"
  
  → markdown-rendered URLs become reading-aloud-eligible
  content. Also corpus-recording-eligible. Also potentially
  the foundation for a "save to my knowledge base" feature
  (zettelkasten direction).

- [x] ~~**Folder picker emoticons changed across builds;
  stabilize.**~~ Landed v0.1.455. Root cause: picker identicons
  were seeded by FULL path; now seeded by BASENAME (_glyphHash) so
  a same-named folder gets a stable mark across location + builds.
  Original note:
- [ ] (resolved) User feedback:
  > "hmm looks like same folders have different emoticons in
  > file picker than from a few test builds ago... might be
  > something to stabilize"
  
  We probably hash filename → emoji and the hash function
  changed at some point. Or we changed the emoji set. Either
  way, make this deterministic per-path so users build
  spatial memory ("Code folder always has 💻").
  
  Touchpoints: file picker emoji assignment. Use a stable
  hash (e.g. blake3 of basename, modulo emoji palette size,
  fixed palette). **trivial.**

- [x] **S — "Mh mhmmm" treated as command; should be
  corpus-decided.** **Landed v0.1.267.** New corpus-driven
  filler-learning system: `register_rejection` bumps a counter
  per normalized utterance; auto-promotes to confirmed filler
  after 3 rejections. Voice path checks `is_filler` BEFORE
  parsing — confirmed fillers short-circuit with a brief
  toast, no popup. Storage: `~/.config/web2qml/filler_words.toml`,
  human-editable, capped at 200 confirmed / 500 pending. 5-word
  max ensures real commands don't get learned as filler.
  Settings UI for management and one-tap "this is filler"
  button on popup remain filed for follow-up.

#### Discovered from log (user may not have noticed)

- [x] ~~**S — `Unable to assign [undefined] to bool` warning
  spams stderr on every page load.**~~
  **Landed v0.1.250.** Root cause turned out to NOT be the
  `_root` guard. The wrapper Item's `_selected` property
  resolved fine. The warnings were firing on DOWNSTREAM
  bindings inside animations: `SequentialAnimation on opacity
  { running: parent._selected }` — `parent` inside the
  animation resolves to the Rectangle (the animation's
  parent in the object tree), not the wrapper Item. Fix:
  gave wrapper Item explicit id `_pwrap_{pid}` and switched
  all inner bindings to use the id directly.

- [x] **trivial — `id is not unique` warning on wikipedia.**
  **Landed v0.1.264** — root cause was NOT paragraph_id (those
  are atomic-counter unique). It was `emit_adaptive_table`'s
  four hardcoded wrapper ids (`_wideCol`, `_wideInner`,
  `_wideHeaderRow`, `_narrowCol`) that didn't include the
  table's tid. Pages with multiple tables emitted duplicate
  ids. Fixed by suffixing each with `_<tid>`.

- [x] **trivial — `qt.text.font.db: OpenType support
  missing` runs thousands of times on wikipedia/apple
  pages.** **Landed v0.1.271.** Prepended
  `qt.text.font.db.warning=false` to `QT_LOGGING_RULES` at
  startup. Targeted — only silences this channel. User-set
  QT_LOGGING_RULES appended after ours, so users can
  override by setting `qt.text.font.db.warning=true`.

- [x] **trivial — `qt.qpa.input.methods: failed to create
  compose table` for en_DK locale.** **Landed v0.1.272.**
  Same QT_LOGGING_RULES prepend pattern as v0.1.271. Other
  Qt diagnostics unchanged; user can override if desired.

- [ ] **observed — "Allison" speaker auto-registered.** Log
  shows `[speakers] registered "Allison" (#2 total)`. Either
  another person was speaking in the environment OR our
  speaker classifier mis-assigned some of the same-speaker
  audio to a new speaker. Worth keeping an eye on — if it
  happens repeatedly with only one human present, our
  embedding similarity threshold is too tight (we should
  merge speakers more aggressively).

#### Deferred per user

- **Dropdown / long-press for voice refinement.** User said:
  > "we should make sure to have a dropdown or button tap
  > and hold to select from list... wait on this one though
  > because we are still analyzing if the system can do it
  > automatically... if I keep bringing it up its a sign
  > something is off in the future not enough data points yet"
  
  Filed for awareness but not acting on. The right signal to
  ship this is recurring user frustration with auto-resolution
  failures. Not there yet.

#### Sequencing recommendation

After this round of filings:
1. **v0.1.250 (immediate, this session):** the `_root → root`
   QML fix. Tiny, eliminates log spam, no UX risk.
2. **Next focused ships, in priority order:**
   - Two-color-speakers fix (small, immediate UX win)
   - Voice popup waiting-state bottom overflow (small, UX)
   - URL bar landscape-mode top default (small-medium)
   - Folder picker up-folder (small)
   - Markdown rendering for `.md` URLs (medium, big content win)
   - Corpus-decided filler words (small-medium)
3. **Larger investigations:**
   - Helix startup latency (combine with TP4 Konsole baseline)
   - Helix tap-to-position (SGR mouse mode full impl)
   - Markdown tables — relaxed grammar
   - Settings icon antialiasing (visual diff first)
4. **Pre-filed arcs** (NIXFIX, SNLP, PKGCACHE/PKGUI/OFFLINE,
   PA2 silence-detection) — still queued behind this round.

### v0.1.247 audit findings — pass 4

Additional findings from continued auditing. None are bugs that
need immediate fixing; filed for future intentional work.

- [x] **S — Archive doesn't persist until shutdown; combines with
  panic=abort for real data-loss risk.** **Landed v0.1.286.**
  `archive_page` now saves on every call (was every 5).
  Matches the persistence pattern of other state modules
  (token_stats, confusion_table, llm_cache, decision_cache,
  speakers, chat). Trivial disk cost (~25KB per write,
  user-action-triggered not in a loop).

- [x] **S — Corpus recorder spawns writer thread on stop; panic
  during the window between spawn and write completion would
  lose recording.** **Landed v0.1.287 (shutdown fence half).**
  Added `corpus_recorder::flush_writer()` that joins the
  pending writer thread, called from `Backend::drop`. The
  spawned JoinHandle is now stored in a static
  `pending_writer()` Mutex so a clean shutdown waits for
  the disk write to finish before the process exits.
  
  The panic case is partially mitigated by `panic = "abort"`
  killing the process before Drop runs, so `flush_writer` is
  bypassed; the writer thread is killed mid-write and the
  wav may be partial. The full fix for that is switching
  panic strategy or moving to a synchronous-on-stop write
  path — still filed under the broader panic=abort arc.

- [x] **trivial — `scraper::Selector::parse` not cached in
  `archive::extract_key_points`.** **Landed v0.1.283.**
  Two call sites in archive.rs (extract_key_points and
  extract_text_snippet) now use OnceLock-cached selectors
  via `sel_body_star()` / `sel_body()` helpers, matching the
  pattern in main.rs (sel_meta and friends). Also dropped
  `head|meta|link` from the skip list inside
  extract_key_points since those are unreachable under
  `"body *"` selector.

- [x] **trivial — `scroll_signals::push_sample` doesn't check
  `content_y.is_finite()` before casting to i64.** **Landed
  v0.1.283.** Added `if !content_y.is_finite() { return; }`
  at function start. Bails out before the cast so NaN/Inf
  inputs don't silently pollute scroll signals with bad
  samples.

- [x] **trivial — `voice::ensure_whisper_model_loaded` unwraps
  `path.to_str()`.** **Landed v0.1.283.** Replaced
  `path.to_str().unwrap()` with `ok_or_else` returning a
  clean error message that includes the path. Surfaces
  cleanly at startup instead of panicking on the rare
  non-UTF-8 case.

- [x] **trivial — FilterStats.selectable_paragraphs doc is
  stale.** **Landed v0.1.283.** Doc updated to reflect that
  the counter ticks for headings, lists, tables, blockquotes,
  and code blocks too — not just `<p>` elements.

- [x] **trivial — phoneme-db.txt is generated but no
  signpost.** **Landed v0.1.286.** All three fixes shipped:
  (1) `scripts/gen-phoneme-db.py` now emits a 5-line
  DO-NOT-EDIT banner box at the top of every regenerated
  file. (2) Banner prepended directly to current
  `data/phoneme-db.txt` so it's visible without regen. (3)
  `data/README.md` gained an "Other files (generated, NOT
  user-editable)" section documenting phoneme-db.txt
  alongside manpages/ directory.

- [x] ~~**trivial — `__pycache__/` ships in the zip.**~~
  **Landed v0.1.248.** Ship command now does
  `find /tmp/ship -type d -name __pycache__ -exec rm -rf {} +`
  before zipping. Verified absent from
  web2qml-stage-m.zip.

### Terminal animation arc (v0.1.241 conversation)

Filed as a separate arc rather than folded into the
terminal performance arc, because animation is delight-
work whereas perf is bug-work. Both are valuable; both
have their own justification and timing.

User suggestion verbatim:

> "can we animate the highlights on text lines in TUIs
> like gitui and helix with overlays and can we animate
> bar lines like in dust? any way to know when they
> should grow definitively or is it just a guess that
> we can cache by looking at bash or zsh history and
> asking a local LLM what it thinks of replayed outputs
> on mock data in a vm?"

> "ok yeah that makes sense to just animate all tiles
> and we could also try to propagate for directional
> tiles like we already do for walking dust and tree
> to chain animations growing in"

### Design summary

**Phase 1: cell-update fade animation (universal).**
Every cell that changes gets a brief accent-color
overlay that fades over 200-400ms. No semantic
understanding required. Cursor moves leave a fading
trail, new output lines glow briefly, status updates
highlight, numbers that updated stay highlighted
momentarily.

This works for EVERY TUI uniformly because it's
purely change-detection on the cell grid. No
per-tool patterns needed for the baseline visual.

**Phase 2: directional cascade for related updates.**
When changes arrive in clusters (a tree being drawn,
a list scrolling, output streaming), the natural
temporal order ALREADY produces a cascade because
cells arrive at different times. For TUIs that BATCH
output (dust prints the whole tree in one ms), we
artificially spread the animation start times based
on cell position — top-to-bottom for trees, left-to-
right for bars, etc.

Directional rules per recognized pattern:
- Tree (box-drawing characters in a hierarchy):
  downward cascade
- Horizontal bar (block characters): left-to-right
  sweep with bar appearing to grow
- Selection move (inverse-video row): from previous
  row toward new row
- Spinner: rotational
- Progress percentage: number-roll instead of jump
- Status line update: highlight the changed columns
  left-to-right

**Phase 3: pattern library expansion.**
Hand-coded recognizers for ~20 common patterns covering
the bulk of high-value terminal experiences. Each is
maybe 50-100 lines including detection + animation
rule. Total ~1000-2000 lines.

**Phase 4 (explicitly NOT to be built):** LLM-in-VM
auto-detection of arbitrary TUI patterns. The user
sketched this as a possibility but the win-to-cost
ratio is bad — semantic understanding via LLM is the
wrong tool when we can do most of the work via cell-
update detection (Phase 1) and a small pattern library
(Phases 2-3). Skip unless real usage proves the
pattern library insufficient.

### Honest scope check

- Phase 1 alone is maybe 200-300 lines of Rust +
  minor QML. **M**. Modify pty_term.rs to track per-
  cell change timestamps; QML painter reads timestamps
  and applies fade overlay. Works for ALL TUIs without
  any per-tool understanding.
- Phase 2 needs a directional-staggering pass on cells
  arriving in a single update batch. **M-L**.
- Phase 3 is 10-20 individual pattern ships, each S-M
  sized. Can be done incrementally — ship a few high-
  value patterns first (dust bars, gitui selection),
  add more based on what the user actually uses.

### Concerns to flag

- **Animation in a TUI can be distracting if overdone.**
  Fade durations short (<300ms), colors subtle, opacity
  bumps small (~10-20% of theme accent). Off-by-default
  toggle in settings. v0.1.226's lessons apply.

- **Performance budget.** Large output (tail -f noisy
  log) can produce 50+ updates per second. Animation
  queue must be bounded — only animate the last N
  updated cells, drop older animations. Probably 100-200
  cell limit is enough.

- **Wayland/KWin compositor issues already flagged.**
  Per-cell animated overlays could exacerbate
  transparency flicker. Real hardware testing required.

- **Composing with TUI's own visual choices.** helix
  has cursor animation, gitui has its own selection
  styling. Layering our animations on top might combine
  awkwardly. Off-by-default opt-in feature.

### Dependencies / sequencing

- Phase 1 standalone (no deps)
- Phase 2 builds on Phase 1's timestamp infrastructure
- Phase 3 builds on Phase 1+2; each pattern its own
  small ship

- Could share machinery with the terminal-performance
  arc's Option G (tile grouping). Tiles ALREADY have
  per-tile invalidation timestamps that the fade
  animation needs. If tiling lands first, terminal
  animation is cheap to add on top.

- Could compete with terminal-performance work for
  rendering budget. Heavy animation + heavy TUI =
  worst case for current QML cell model. Probably
  shouldn't land until either tile grouping (Option G)
  or other perf fixes have happened.

### Why this is filed separately from terminal perf

The two arcs have different motivations and could be
done independently:
- Terminal perf: bugs (slow vs Konsole), shipped to
  fix something broken
- Terminal animation: delight (feels alive, reveals
  state changes visually), shipped to ADD something
  appealing

They share infrastructure (per-cell change tracking,
tile rendering) but the question "do we add fade
animations?" is independent of "do we fix the perf
gap?"

Sequence-wise: probably perf fixes (or at least their
profiling) land first, then animation, since broken
trumps shiny.

### Songs / sound / audio + onboarding ideas filed elsewhere

Captured under the Phoneme alignment + tiered reading
model arc above. The sing-challenge mode (PC1), the
onboarding-as-enrollment variant (PC2), the real-time
letter tracking (PC3), and the lyric-playback-with-
user-voice ideas all live there as PC sub-ships.

The "package instrumental loops + rhyme dictionary +
random sentence challenges" pattern is filed inside
PC1's design. The "people sing what they have heard
as remixes so you still get their natural tones"
observation is preserved as the corpus-data
justification inside PC1.


### v0.1.230 hardware-test feedback (user session, May 2026)

Substantial batch of feedback after running v0.1.230 with the
voice fixes from v0.1.224-225 working. Highlights:

**Validated as working:**
- "the corpus is already helping me improve" — v0.1.225 VAD
  fix landed, captures are producing fingerprint data, the
  feedback loop works
- Wake-listen + whisper alive (logs show "whisper said: test")
- Corpus listing reachable (the user got far enough to see it)

**Bugs surfaced (high → low):**

- [~] **close-helix closes voice prematurely.** v0.1.458:
  executeIntent called close() BEFORE the term_cmd dispatch, so
  onClosed (cancel-listen/resume-wake/restore-focus) ran before
  the :q\r reached the PTY. Now deferred via Qt.callLater (guarded
  against the llm re-open case) so command flushes first, then
  popup closes. AWAITING hardware confirm. Original note: When the
  user closes helix (running inside the terminal tab), the
  close signal gets passed to "close" which closes voice
  before it has a chance to quit the pager. Cleanup ordering
  bug. Voice shouldn't terminate as a side-effect of an
  in-terminal app exit. Need to scope the close signal to
  the terminal pane (or the buffer in helix), not the whole
  voice runtime.

- [ ] **M — Transparency flickers until app restart.**
  User reports: "transparency is causing flickers
  everywhere until you restart the app." Likely a
  Wayland/KWin compositor issue with Qt's translucent
  windows — Qt's QQuickWindow alpha can produce
  rendering artifacts when the compositor doesn't
  fully handle the surface format we're requesting.
  Possible mitigations:
  - Set `Qt.application.styleHints.flags` to include
    proper window-translucency hints
  - Force `QSurfaceFormat` with alpha buffer at startup
    (via build.rs or main.rs initialization, before
    QGuiApplication construction)
  - Use Qt's `requestedAlpha` flag explicitly
  Needs Asahi-Fedora hardware testing to characterize
  the actual artifact (single-frame flash on resize?
  ongoing tearing? compositor-specific?).

- [ ] **M — Background blur doesn't apply.** User
  reports: "background blur effects don't apply."
  Background blur isn't a standard Qt feature. Each
  platform requires its own window-manager hint:
  - KDE (KWin): `_kde_blur_behind_region` X11 property
    or Wayland's `org_kde_kwin_blur` protocol
  - GNOME: no public API; rumored mutter extension
  - macOS: `setAutomaticallyAdjustsBlendIn` on NSWindow
  Qt doesn't expose any of these directly; needs
  platform-specific code via `QWindow::setProperty`
  or the platform plugin's native handle. Significant
  per-platform work. File as M-with-stars.

- [x] ~~**S — Terminal tab gets no transparency.**~~
  Fixed v0.1.237. Two terminal-pane Rectangles
  (the outer SwipeView pane at line ~1822 and the
  inner `termScreen` at line ~1854) were both using
  `color: th.tbg` — fully opaque, hiding everything
  behind the window even when `bg_opacity` < 1.0.
  Both now use `Qt.rgba(tbg_r, tbg_g, tbg_b, bgOp)`
  — same pattern as the root ApplicationWindow's
  background. The terminal keeps its themed darker
  tint (tbg vs bg) but becomes translucent when
  bgOp is reduced. Other terminal UI bands (touch
  row, command suggestions) remain opaque — they're
  foreground elements and the user wants them
  readable.

- [x] ~~**S — QML SwipeView binding loop on currentIndex.**~~
  Fixed v0.1.231. Replaced `currentIndex: tabBar.currentIndex`
  one-way property binding (which collided with SwipeView's
  internal ListView binding on the same property) with an
  imperative `Connections` handler on tabBar plus
  `Component.onCompleted` to align initial state. The
  existing `_syncingTab` guard still protects the back-write
  path. No more "Binding loop detected" warnings.

- [x] ~~**S — Back button to corpus doesn't reload.**~~
  Fixed v0.1.231. `render_cached` now special-cases
  `web2qml://corpus` before the cache lookup — regenerates
  fresh markdown rather than falling through to a cache miss
  (which previously left the stale preview in place).

- [x] ~~**S-M — Weird characters rendered in corpus page.**~~
  Defensively fixed v0.1.231 (validation needed on hardware).
  The paragraph-text display path in `list_recordings_markdown`
  now strips control characters (NUL, vertical tab, ANSI
  escapes, etc) and bidi/directional formatting marks (U+200E,
  U+202A-E, U+2066-9). CR/LF/tab become spaces. If the weird
  chars were something else (font-missing glyph, broken UTF-8),
  this filter won't catch them — re-test.

**Reading-overlay redesign (continuation):**

- [ ] **M — Reading-overlay sub-ship A' (inline, eats paragraph).**
  User said: "could you make that popup at the bottom become
  an inline that appears at paragraph bounds and slowly eats
  it up." That's a different visual paradigm than the
  v0.1.226 5-word fade window — instead of a SEPARATE bottom
  bar showing the words, dim the paragraph itself as it's
  being read, "eating up" the text in place. Replaces the
  overlay entirely with an in-text effect. Bigger redesign
  than v0.1.226 was; supersedes the current overlay.

- [x] ~~**S — Slightly slower default reading rate.**~~
  Landed v0.1.231. WPM 180 → 160 (=375ms/word).

- [x] ~~**S — Speaking-rate slow/med/fast tap-hold button.**~~
  Landed v0.1.234 (tap-cycle subset). Tap button cycles
  slow(130) → med(160) → fast(200) → slow. Button visible
  in both "ready" state header (set rate BEFORE starting)
  and "reading" state bottom row (adjust mid-read). Shows
  current preset's label. Long-press for fine-control
  slider filed as sub-ship below.

- [x] ~~**S — Tap-and-hold WPM slider.**~~ Landed v0.1.238.
  TapHandler's `onLongPressed` on both rate buttons (ready-
  state header + reading-state bottom row) toggles a Slider
  popup. Slider sits above the readingOverlay (negative y)
  with range 80-280 WPM, snap to 5-WPM increments, live
  preview (wpm updates as user drags so timer interval and
  visual reflect new rate immediately). Persistence happens
  on slider release (`onPressedChanged: if (!pressed)
  persistWpm()`) to avoid dozens of file writes per drag.
  Dismissal: long-press the rate button again (toggle).
  Considered tap-outside-to-dismiss but the catch area
  conflicted with the readingOverlay's existing buttons —
  simpler model wins.

- [x] ~~**S — Persist WPM across launches.**~~ Landed v0.1.238.
  Two new qt_methods: `reading_wpm_get() -> i32` and
  `reading_wpm_set(wpm: i32)`. Storage: a single-line
  integer in `~/.config/web2qml/reading_wpm`. Sanity-clamp
  to 80..=300 on read (corrupted file = use default 160)
  and on write. QML reads via `Component.onCompleted` and
  writes from `cycleRate()` + slider release. Used the
  one-shot-file pattern (palette.txt, wake_phrase.txt)
  rather than extending the 13-tuple settings.conf
  loader.

**Feature requests (planned arcs):**

- [ ] **M — Smart buffer scanning ("jump to function").**
  User linked https://docs.helix-editor.com/commands.html
  as inspiration. Idea: "jump to this function" voice
  command emulates a few pathways based on state tracking
  (is user focused, is user in edit mode). Maps voice
  intent to a helix-like static command catalog. Hooks
  into the existing voice-intent system but adds buffer
  awareness (current paragraph / function / table / etc).

- [x] ~~**M — Tap-and-hold theme picker.**~~ Landed v0.1.239.
  Added 12 new themes (Reading: ReadDk, ReadLt, Sepia, News;
  Red-tinted: NightR, Crimsn, Amber, Ember; Focus-contrast:
  HC-Dk, HC-Lt, Slate, Paper) to the QML themes array,
  growing the total from 8 to 20.
  
  Picker UX:
  - Default view: 8 base themes in a 4-column grid (2 rows).
  - Header hint: "(long-press to show more)".
  - Long-press any tile → grid expands to 5 rows (20 themes),
    header switches to "(long-press tile to collapse)".
  - Long-press a tile in expanded view → collapses back.
  - Tap selects (works in both modes).
  - Each tile shows a tiny 10×10 color swatch on the right
    edge (the theme's bg color, bordered with its accent
    color) — visual preview cue that's important when many
    themes share name prefixes (Read*, HC-*).
  - Quick-settings popup is inside a Flickable with vertical
    scrollbar, so the expanded grid scrolls if it overflows
    the visible area.
  
  Light/dark detection now uses perceived luminance (BT.601:
  0.299R + 0.587G + 0.114B, threshold 128) rather than
  hardcoded `index !== 1`. Replaces both the QML tap handler
  and the boot-time Rust init. Generalizes across arbitrary
  future theme additions without index churn.
  
  `themePickerExpanded` is session-only — no persistence.
  Three new readonly properties: `themesBaseCount=8`,
  `themesExpandedCount=20`, and the expanded flag.

- [x] ~~**System light/dark watcher.**~~ Landed v0.1.423.
  src/system_theme.rs detect() reads GNOME gsettings color-scheme
  + KDE kdeglobals (scheme-name, luma fallback), 9 tests.
  backend.system_color_scheme() qt_method; QML "Follow system
  light/dark" opt-in toggle + 3s poll watcher that flips dark/
  light only on an actual system change (doesn't fight a manual
  pick). D-Bus subscription deferred — polling is simpler and the
  3s cadence is fine.

- [ ] **M — More helix bindings throughout the app.**
  Beyond terminal-tab helix, the global keymap could
  borrow more helix conventions (selection-then-action,
  g-prefix navigation, space leader). Audit current
  shortcuts against helix's commands.html and pick the
  high-value ones.

- [ ] **L+ — Tobii eye tracker integration.** Theme-matched
  highlight circle shows where closest hit will land OR
  where closest reference is coming from (in terminal or
  browser). Big idea — depends on Tobii SDK availability
  on Linux/Asahi and may need user-driven hardware setup.
  File for now; revisit if user actually has the hardware
  and a clear use case.

**Voice-corpus quality:**

- [x] ~~**M — Paragraph-selection filter too tight.**~~
  Landed v0.1.236. Extended `wrap_for_paragraph_selection`
  to four more block types in qml_gen:
  - `<h1>`, `<h2>`, `<h3>` (h4-h6 excluded as inline labels)
  - `<ul>` / `<ol>` top-level lists (nested inherit outer wrap)
  - `<table>` top-level (cells joined with spaces, rows
    with FRACTION SLASH separator)
  - `<blockquote>` (uses standard prose paragraph bounds)
  
  New `extended_word_bounds()` returns (3, 200) — wider than
  `paragraph_word_bounds()` (10-50) because tables/lists are
  intrinsically longer. Selection text content built
  appropriately per type so the reading flow gets a natural
  string. Manpage tables/lists/headings (separate pipeline)
  not yet covered — filed below.

- [x] ~~**M — Manpage tables/lists/headings selection.**~~
  Landed v0.1.240. Same wrap pattern as v0.1.236 (qml_gen)
  applied to the manpages markdown→QML pipeline. Three
  block types now wrap with `wrap_for_paragraph_selection`
  when their text content fits the extended bounds
  (3..=200 words):
  
  - **Headings (h1/h2/h3)**: plain-text payload is the
    heading text with inline markup stripped. h4-h6 stay
    unwrapped (they're usually inline labels like "Note:").
  - **Lists (- and 1.)**: collected items into a temp buffer
    first, then wrapped. Plain-text payload = items joined
    with " · " (matching qml_gen's convention).
  - **Tables**: built from PRE-RICH `data` (cells with
    inline markup stripped). Headers + each row joined
    with spaces, rows separated with " ⁄ " (FRACTION SLASH).
  
  Blockquotes have no manpage support (markdown `>` syntax
  isn't parsed). Skip.
  
  All four built-in manpages (welcome, manpages, onboarding,
  the auto-generated corpus virtual page) now expose their
  structured content for read-aloud capture. Particularly
  useful: the corpus virtual page's per-speaker recording
  TABLE is selectable, so the user can read their own
  corpus list aloud as another capture (a meta moment).

- [x] ~~**S — Add 300ms delay to reading start (animations
  + backend).**~~ Landed v0.1.236. User: "could you also
  add a short delay to the start of the reading both on
  the animations and the backend so that the user can
  figure out what is going on... maybe a 300ms delay."
  Implementation: state transitions to "reading"
  immediately (overlay slides in) but `currentWordIndex=-1`
  initially. A 300ms one-shot timer
  (`_startDelayTimer`) then sets `currentWordIndex=0`
  AND calls `corpus_start`. WPM advance timer gated on
  `currentWordIndex >= 0` so it doesn't tick during the
  delay. Pre-delay visual: words 0/1/2 faintly visible
  (the opacity formula's natural behavior with index=-1)
  → crossfades to first-word-bright when index flips.

- [x] ~~**M — Markdown tables don't look nice yet.**~~
  Landed v0.1.235. Wide-mode table rendering polished:
  - Each data row wrapped in a Rectangle with alt-row tint
    (zebra) for scanability — odd rows get a subtle 3-4%
    alpha-tint background, even rows transparent
  - Header row gets a slight tinted background (`input_bg`)
    and a 2px separator below (was 1px) for stronger
    hierarchy
  - Cell padding via wrapper Rectangle's
    `height: <inner>.height + 12` (~6px top + 6px bottom)
  - Outer container gained a thin 1px border + radius 4 for
    unified grouping (was a bare Item)
  - ColumnLayout spacing 2 → 0 (zebra rows handle row
    separation, gaps would break the alternation)
  - New `zebra_bg` field on RenderStyle, themed per
    dark/light (white-with-alpha vs black-with-alpha)
  Narrow-mode (stacked cards) already looked decent and
  was left alone — focus was on the wide-mode utilitarian
  rendering the user complained about.
  4 new tests covering outer border, zebra emission,
  cell padding, _wideCol id.

- [ ] **L — Standard table format OR multi-match ranking
  by cleanliness.** User: "my example file where they
  are not prettified" — non-canonical markdown tables
  (unaligned columns, missing separators, ragged rows)
  fall through current parsing. Two strategies: (1) be
  more permissive in `parse_markdown_table`, accept
  ragged input; (2) rank multiple parse candidates by a
  "cleanliness" heuristic (column-count consistency,
  separator presence, etc) and pick the best. (2) is
  more ambitious; (1) is the smaller first step.

- [x] ~~**S — Default depth = 15.**~~ Superseded /
  resolved by v0.1.399. This older request ("default-depth
  change to 15") had ambiguous scope. The user later
  clarified (mindmap-feedback turn) that the relevant depth
  is the recursion depth feeding `generate_safe_qml`, and
  asked for 50 rather than 15. v0.1.399 set all three depth
  defaults (config-internal, no-config fallback, CLI) to 50
  and fixed the latent CLI-clobbers-config bug. The "15"
  number is obsolete; 50 is the current default.

- [x] ~~**S — Homepage / top-bar focus appearance check.**~~
  Fixed v0.1.232. Interpreted as: when navigating (homepage
  or anywhere) OR focusing the URL bar via shortcut/voice,
  the chrome should reveal itself if it was scroll-hidden.
  Two hooks added:
  (1) `Connections { target: backend; function onLast_query_changed() }`
  at root level — any nav resets `urlBarVisible = true`,
  unwedging the scroll-hidden state
  (2) `onActiveFocusChanged` on urlBar — when the bar gains
  focus (Ctrl-L, click on pill, voice command), forces
  `urlBarVisible = true` so the cursor is actually visible.

**Storage / encoding:**

- [x] ~~**M — Opus encoding instead of WAV (ASAP per user).**~~
  Landed v0.1.231 (validation pending on hardware). After
  `write_recording` finishes the WAV write, a `convert_wav_to_opus`
  helper shells out to ffmpeg with `-c:a libopus -b:a 32k
  -application voip`. On success, deletes the WAV. On failure
  (no ffmpeg on PATH, encoder error, undersized output), keeps
  the WAV — the lister handles both extensions transparently.
  No new compile-time deps. ~22x size reduction (88KB/s WAV
  → 4KB/s opus at speech-quality bitrate). The capture path
  itself stays WAV-only so the audio thread never blocks on
  encoder errors. Filed as M; could be promoted to M+ if user
  reports issues.

### v0.1.223 hardware-test feedback (user session, May 2026)

User ran v0.1.223 on the Asahi MacBook through the full
onboarding registration flow. What worked, what broke, and what
needs redesign — captured fresh from the run log before details
fade.

### v0.1.224 hardware-test follow-on (user session, May 2026)

User ran v0.1.224 right after. Three immediate items surfaced
plus one larger arc proposal:

1. **One-gesture paragraph→read.** User: "when the paragraph
   is selected I don't want to have to then open the mic button
   to speak I want to just be able to start reading."
   → Fixed v0.1.225. Paragraph select now auto-arms the
   reading overlay (jumps idle→reading, fires corpus_start).
   The persistent voice stream is already open; corpus_start
   only flips the "save these bytes" flag, so no new mic
   capture is being initiated. Cancel button in the overlay
   still works to abort.

2. **"no arecord or whisper in path" popup on cancel.** User
   tapped the X button in voice popup → got the external-tool
   fallback error. The cancel path was being treated as a
   failure and routed to listen_external. → Fixed v0.1.225.
   `record_and_transcribe` now distinguishes cancellation
   ("cancelled by user" in error message) from real errors
   and returns empty result instead of falling back.

3. **VAD still rejecting voice.** Root cause found:
   stereo-interleave bug in the ZCR computation. The 100ms
   window read L0 R0 L1 R1... as a continuous mono sequence,
   producing spurious zero-crossings at every channel
   boundary. Real speech ZCR (50-300 per 100ms) was inflated
   to 2100-3000, blowing past the 30..800 voicing gate.
   → Fixed v0.1.225. ZCR loop now strides by source_channels
   (left channel only). The 30..800 band is now correctly
   interpreted as per-channel ZCR per 100ms. Same ship also
   adds `WEB2QML_VAD_DEBUG=1` env var for per-chunk tracing
   when tuning thresholds against new hardware.

#### Working

- Folder icons render correctly (filing v0.1.217-ish work
  validated in the wild).
- Onboarding popup auto-trigger fires (v0.1.219 worked).
- Manpage reading via paragraph selection still functions —
  paragraphs p_0 through p_5 selected in sequence as user read
  the onboarding doc.
- Whisper Base loads in ~29ms (q5_1 quantized, ~59MB).
- Speaker registration completes — `[speakers] registered
  "Kessler DuPont-Teevin" (#1 total)`.

#### Broken — persistence not wired into onboarding flow (HIGH)

- [x] ~~**M — Onboarding should enable WEB2QML_PERSIST_LEARNING.**~~
  Landed v0.1.224. The act of registering a voice IS opt-in by
  definition, so `speaker_register_current` now calls
  `security::enable_persistence_runtime("voice registration")`
  on the success path. This:
    1. Flips the in-memory `PERSISTENCE_FLAG` atomic to true,
       so subsequent worker saves succeed mid-session.
    2. Writes `~/.config/web2qml/persistence_enabled.flag` (a
       zero-byte stamp — mirrors the v0.1.219 `onboarding_seen.flag`
       pattern) so the NEXT session starts with persistence on
       without env-var gymnastics.

  Storage refactor: replaced the `OnceLock<bool>` cache in
  `persistence_enabled()` with an `AtomicBool` that can be
  flipped at runtime. Initial value: env var = "1" OR stamp
  file exists. Logged source in the boot message so the user
  can tell whether they're on env, stamp, or both.

  User can opt back out by deleting the stamp file. No
  settings UI yet — filed L for a quick-settings toggle.

#### Broken — VAD over-rejecting voice (HIGH)

- [x] ~~**M — VAD threshold too aggressive on this hardware/mic.**~~
  Diagnosed + fixed v0.1.225. The root cause turned out to be
  not threshold tuning but a **stereo-interleave bug** in the
  ZCR computation. The 100ms window for VAD analysis read
  interleaved stereo (L0 R0 L1 R1...) as a continuous mono
  sequence. Independent L/R signals create spurious
  zero-crossings at every channel boundary, inflating ZCR
  values 5-20x. On the user's MacBook (44.1k stereo) real
  speech ZCR of ~50-300 per 100ms was inflated to 2100-3000,
  blowing past the 30..800 voicing gate. Every chunk got
  rejected as "non-voice noise."

  Fix: the ZCR/RMS computation now strides by source_channels,
  reading only the left channel. Comment math also corrected
  (was inconsistent about "per sec" vs "per 100ms").

  Same ship: added `WEB2QML_VAD_DEBUG=1` env var that emits
  per-chunk rms/zcr/gate/decision lines. Lets the user (or
  future-Claude reading a transcript) tune thresholds against
  real captured audio. Mid-term adaptive ZCR threshold from
  first-N samples is still a separate item below.

- [ ] **L — Adaptive ZCR threshold based on first-N samples.**
  v0.1.225 fixed the stereo bug; the 30..800 band is now
  correctly interpreted as per-channel ZCR per 100ms. But the
  fixed band still won't fit every hardware/speaker combo
  perfectly. Future improvement: after first 3-5 accepted
  voice samples, learn the user's ZCR center and broaden the
  band ±2σ instead of using a fixed range. Defer unless real
  users hit edge cases the fixed band misses.

#### Broken — whisper misrecognition (LOW priority — model choice)

- [ ] **L — Consider switching default whisper model.** The
  user has `ggml-base.en.bin`, `ggml-tiny.en.bin`,
  `ggml-base.en-q5_1.bin`, `ggml-tiny.en-q5_1.bin`, and
  `ggml-large-v3-turbo-q5_0.bin` (547MB!) in their models dir.
  The base.en-q5_1 (~59MB) is the active default. Many
  transcription errors visible in the log:

  | Read aloud (from manpage)                    | Whisper output                                |
  | -------------------------------------------- | --------------------------------------------- |
  | "the voice registration flow"                | "this is the voice registration flow" ✓       |
  | "system will create a voice profile…"        | "people are in the room" ✗                    |
  | "and read the next paragraph"                | "right on the screen and read the next…" ✓    |
  | "she sells seashells by the seashore"        | "jumps over the lazy dog, she sells…" ✗       |
  | "the audio and suggest a color"              | "and" then "suggestion pick a different…" ~   |
  | "command page → next for the list"           | "command page, and next for the list…" ~     |

  Some are model error; some are VAD chunk-boundary errors
  (partial words on the edges). The large-v3-turbo model is
  on disk but unused.

  Two paths: (a) auto-select highest-quality available model
  at startup, (b) user-facing model picker in quick settings.
  Defer unless user wants to optimize.

#### Broken — phonetic DB empty (CONSEQUENCE of persistence bug)

- [x] ~~**M — Phonetic match fallback when DB is empty.**~~
  Done v0.1.882: the per-lookup "pitch index miss → full scan"
  line is now suppressed when the fingerprint gallery is empty (a
  full scan of zero fingerprints is a no-op, so the log was pure
  noise on fresh installs), and the loader prints a one-time
  "fingerprint DB empty — speaker matching disabled until voices
  are enrolled" at startup so the user knows the feature is dark.
  Original note:
  Logs show `[phonetic] pitch index miss for bin N, falling
  back to full scan` consistently — but the full scan also
  has nothing to match against because the DB never persisted
  (see persistence bug above). When the DB is empty, this
  fallback noise is mostly wasted log spam. Once persistence
  is fixed (or as an independent improvement), suppress the
  "miss" log when the index is empty AND log a one-time
  "phonetic DB empty — speaker matching disabled" message at
  startup so the user knows the feature is dark.

#### Design feedback — reading-overlay flash animation (M)

- [ ] **M — Redesign reading-overlay word highlight.** Current
  implementation (ship 2.2, `readingOverlay` in main.qml around
  line 6261) is a Flow of per-word Texts; a Timer ticks every
  240ms (250 WPM default) advancing `currentWordIndex`. The
  current word is bold + accent-colored; past words are dimmed
  gray; future words normal.

  User feedback verbatim, captured from session:
  - "the animations on the bottom are too fast and don't quite
     follow the help reading"
  - "they are too distracting because they are colored"
  - "maybe we need to rework it to be max five word buffer that
     1 or 2s fades brighter and start matching on syllables as
     well as words"
  - "maybe the timing on the words is off and needs more
     alignment passes and greedy analysis to better fit phonetics
     especially on fast speakers with lot's of speaking variation
     rate and tone (the proper way to speak in some cultures)"

  Design redirection:
  1. **5-word buffer**, not full-paragraph display. Show only a
     sliding window of ~5 words centered on the current reading
     position. Reduces visual noise and makes the highlight
     less of a "speed reader" effect.
  2. **1-2s fade** instead of instant advance. Words enter at
     full brightness and fade out over 1-2s, replaced by the
     next. Smoother visual rhythm matches actual reading pace.
  3. **Subtle color**, not high-saturation accent. Cooler
     palette so the eye isn't drawn to chase the highlight.
     The reader should look at the paragraph (which they ARE
     reading) and use the overlay as ambient confirmation, not
     a focal point.
  4. **Syllable-aware matching**, not just word boundaries.
     For fast speakers, partial-word matches give finer-grain
     alignment. Need a syllabifier (English first; latin-1
     simple rules: vowel groups separated by consonants).
  5. **Alignment passes**: instead of a fixed-WPM advance,
     run a continuous-time alignment between the incoming
     whisper stream and the expected text. Greedy left-to-right
     match with backtracking on conflict. Existing alignment
     code in the corpus recorder might be repurposable.
  6. **Rate adaptation**: detect speaker WPM from first ~3s of
     audio and adjust the visual rate. Default 250 WPM is
     wrong for many cultures and individual speaking styles
     (some languages average 200, others 300+).

  This is a larger arc, not a single ship. Suggested
  sub-ships:
  - [x] ~~Sub-ship A: 5-word buffer + slower transition~~
    Landed v0.1.226. Replaced bold + high-saturation accent
    color with an opacity-based 5-word window. Each word's
    opacity is bound to its offset from `currentWordIndex`
    (center=1.0, ±1=0.55, ±2=0.25, beyond=0). `Behavior on
    opacity` with 1200ms NumberAnimation crossfades adjacent
    words smoothly. Single text color (theme `tc`), no bold —
    brightness does the work, no hue change. Default WPM 250
    → 180 (=333ms per word, matches relaxed reading). The
    Flow still holds all words; no scrolling logic. Long
    paragraphs that wrap to multiple lines still wrap
    normally — the active window moves through them.
  - [ ] Sub-ship B: rate adaptation (measure user's actual
    WPM, update overlay rate).
  - [ ] Sub-ship C: syllable-aware matching + alignment
    passes (the real work — adds an alignment pipeline
    alongside whisper).

#### Bug — QML startup warnings

- [x] ~~**S — `Unable to assign [undefined] to QColor` at main.qml:1334.**~~
  Landed v0.1.224. The expression was `color: th.tx` — a typo;
  the theme dicts use `tc` for text-color, not `tx`. Fixed to
  `color: th.tc`. (Not a startup race after all — pure key
  typo. Worth knowing for the next "undefined to QColor"
  diagnosis.)

- [x] ~~**S — `Unable to assign [undefined] to bool` for paragraph
  `_selected` property.**~~ Landed v0.1.224. The expression
  `_root._selectedParagraphId === _pid` evaluated to undefined
  when `_root._selectedParagraphId` wasn't set yet (component
  instantiated before root properties resolve). Fixed in
  `qml_gen::wrap_for_paragraph_selection` to:
  ```
  property bool _selected: (_root && _root._selectedParagraphId === _pid) === true
  ```
  The `=== true` coerces any undefined back to false. Removes
  one warning per paragraph per render — visible cleanup of
  the log.

#### User reminder — git work not yet visible

User noted: "I don't see any of the git work yet." Referring
to the voice-corpus → git substrate arc (the foundational
work for moments, attribution, kid-vs-parent multi-user).
That arc is on the todo (`Markdown editor + git versioning
arc` and the larger personal-computing-substrate direction at
the top of this file), not yet started. Confirmed still on
the active list; no immediate work scheduled this session.

### Multi-user collaboration

The active focus area. v0.1.117 landed live multicolor view + speaker
circles. v0.1.119 landed per-chunk Separator wiring, register UI, and
hard whitelist gating. v0.1.120 landed per-speaker audio routing
via pool. Remaining work:

- [x] ~~**S — "+ another speaker" affordance**~~ — landed v0.1.127.
- [x] ~~**M — Time-aligned word seating across speakers**~~ — landed
  v0.1.128 via audio_start_ms threading through Job/Result/PreviewWord.
- [x] ~~**S — Diff-based per-speaker replacement**~~ — landed v0.1.128:
  prefix-matching against the existing speaker block preserves
  arrived_at_ms for unchanged words; only the divergent tail re-fades.
- [ ] **M — Hand-touch handoff gesture.** Physical screen tap claims
  the floor. Mechanism: tap your own circle = "I'm driving now".
  Visual feedback: handed-off-to circle gets a brief border flash.
- [ ] **M — Observer mode.** Allowed=true but doesn't drive commands;
  voice still appears in the transcript for context. Useful for
  silent participants in a paired session.
- [x] ~~**S — Show transcript of muted speakers in a "background"
  channel**~~ — landed v0.1.131. Muted audio now flows through
  the same per-speaker pipeline; renders italic + 0.45 opacity in
  wordsView. Hard whitelist for command dispatch unchanged.

- [ ] **M — Multi-user registration UX.** When user taps "add new
  user", the registration flow should be deliberate, not a one-shot
  spoken word. User sketched: pause audio capture briefly while the
  registration dialog is open. Capture both a name and a color.
  Name entry options: text field, OR voice ("my name is Alice").
  Color entry: linear RGB picker slider. Reason for pausing audio:
  the dialog itself is a focused interaction and shouldn't compete
  with simultaneous voice command processing.

- [ ] **M — Suggest color from voice's MFCC bins → RGB heatmap.**
  When registering a new user, instead of (or alongside) a manual
  RGB picker, suggest a color derived from the new speaker's voice.
  User's sketch: heatmap base of BLUE-RED mapped from MFCC bin
  amplitudes, plus an OVERLAY of orange-purple for volume, plus
  GREEN for cadence. Mixed result becomes a distinctive per-person
  color that's also informative about voice characteristics. The
  user can accept the suggestion or override with the slider.

- [ ] **L — User merging/diverging when same voice shifts style.**
  User reported: "I was the original user but I then became
  different tone and got kicked out of my voice." This is the
  realistic case of a registered user whose voice naturally shifts
  (sick voice, whisper, on phone, lying down). Current design has
  the identifier fail and treat it as unknown. Needed: a way for
  the SAME person to have multiple voice sub-profiles under one
  identity (covered partially by the "speaker→voices schema
  refactor" filed elsewhere), AND a way to MERGE: when a "new"
  unknown speaker pattern persists alongside the known one and
  shares cadence characteristics, suggest "is this Alice in
  a different mode?" with a confirm. Inverse problem (DIVERGE):
  if a speaker's identification drifts because the EMA absorbed
  noise from another speaker over time, allow splitting their
  fingerprint cluster.

- [x] ~~**S — "Kessler, here, run dust" floor-claim phrase.**~~ Landed
  v0.1.149. `strip_floor_claim_prefix()` in `src/voice_intent.rs`
  matches `<name>[,:] here[,] <rest>`, looks up the name case-
  insensitively in the speaker registry, calls
  `speakers::note_active(name)` on match, and returns the
  stripped remainder. Wired into both `parse()` and
  `parse_candidates()` so downstream intent + candidate
  generation work on the actual command. Unknown names pass
  through unchanged. Unit tests in `floor_claim_tests`: 3
  deterministic negative tests + 3 #[ignore] positive tests
  that require registering "Test" first. The "I'm X" /
  "this is Alice" registration variant is a separate flow,
  not landed in v0.1.149.

- [ ] **L — Spanish phrase support: "ponlo en descargas el ..."
  for rename-to-deletable.** Localized command: "put it in
  downloads as [filename]" → renames the file/folder to
  `to-delete--<filename>` (so the user can later batch-clean by
  globbing the prefix). The grammar parse is non-trivial; needs
  proper Spanish particle handling in the parser. Currently
  voice_lang supports EN/ES/FR for whisper transcription but the
  intent parser is English-grammar. This is filed as L because
  it's the wedge for proper multilingual intent parsing.

- [x] **S — "mira kwin" as Spanish cd analogy.** **Landed
  v0.1.288.** Added `mira X` / `míra X` prefix matcher in
  `voice_intent_es::parse_es` that emits
  `Intent::TerminalCommand("cd X")`. Placed BEFORE the
  open-prefix matchers so "mira" doesn't get misread as a
  generic discourse marker. Strict prefix (not
  normalize-stripped) so legitimate mid-utterance uses
  ("mira aquí", "mira esto") aren't misclassified as cd.

- [ ] **L — User security model.** User flagged: "user security
  is something interesting to think about in the future."
  Currently voice profile is per-user-config-dir; no cryptographic
  binding between identity claim and actual person. Mature design
  considerations: liveness detection (voice replay attacks),
  per-user sandboxing of what commands they can run, audit log
  of who-said-what, optional out-of-band verification (PIN entry
  for sensitive commands). Real research; not a single session.

### Whisper streaming polish

- [ ] **S — Word-level timestamps from whisper.** whisper-rs supports
  per-token timestamps via `set_token_timestamps(true)`. Use these to
  drive precise fade-in timing instead of approximating with poll-time
  arrival.
- [x] ~~**S — Configurable preview cadence.**~~ — landed v0.1.131
  via WEB2QML_PHONETIC_MS / WEB2QML_WHISPER_MS env vars and Settings
  ± buttons. Atomics read per loop iteration so changes apply live.

### Voice pipeline follow-ups

- [ ] **L — Onboarding color override slider.** v0.1.212's
  registration dialog shows a derived color preview but doesn't
  let the user override. Original 2.6.5 filing wanted accept/
  override. If users find the auto-color unsatisfying, add a
  hue slider that re-derives at chosen position, then either
  thread the override through `add_speaker` or call a separate
  `set_speaker_color(name, hex)` post-registration. The latter
  is cleaner since add_speaker's signature wouldn't change.

- [ ] **L — First-launch auto-trigger for onboarding.** v0.1.212
  requires the user to find "register your voice" in
  quick-settings. A real first-launch flow would detect
  speakers.toml being empty (or non-existent) and offer to start
  the onboarding tour on the first frame after startup. Wants a
  Backend method `is_first_launch() -> bool` and QML's
  Component.onCompleted at root.

- [x] ~~**M — tourDriver.stop() should reset onboardingActive.**~~
  Landed v0.1.213. stop() now resets root.onboardingActive
  alongside clearing _script and the bubble connection.
  startOnboarding() compensates by setting onboardingActive
  AFTER calling start() (which always calls stop() internally),
  so the flag set isn't clobbered by the internal cleanup.

- [x] ~~**M — bg-drain intent matching is too permissive.**~~
  Landed v0.1.213. Added `Intent::is_high_confidence()` that
  returns false for catch-all variants (Dictate, Freeform,
  LlmFallback, Unknown, SearchWeb) and true for verb-specific
  matches (Navigate, GoBack, TerminalCommand, etc.). bg-drain's
  known_intent check now uses this instead of the
  "kind != unknown && kind != error" string compare. Overheard
  ambient speech like "where you are, before you go on screen"
  from the v0.1.205 log would have hit Dictate/Freeform and is
  now rejected. 4 unit tests in voice_intent::high_confidence_tests.

- [ ] **M — Whisper-rs vendoring + incremental decoding.** Stock
  whisper-rs 0.16 re-encodes the full audio buffer on each streaming
  tier pass. Vendoring (with patches to whisper.cpp) could let the
  encoder reuse state across appends, dropping per-pass cost
  significantly. Real investigation needed: read current
  whisper.cpp API for KV cache reuse / encoder state survival,
  identify specific patches, decide if benefit justifies the
  maintenance burden of a vendored fork. Pairs naturally with the
  live grayed-out word streaming UX (next arc).

- [ ] **M — Wire cadence into identify() scoring + audit current
  scoring math at the same time.** v0.1.137 collects cadence per
  speaker but nothing reads it. Next time we touch the matcher:
  use cadence as a secondary signal when tonal similarity is
  borderline. Same opportunity: audit the existing
  speaker-similarity formula (currently fixed 0.65 threshold, no
  style awareness). User's "scoring overhaul opportunistically"
  framing.

- [ ] **XS — Upgrade cadence timing from per-segment to per-token.**
  v0.1.137 distributes each whisper segment's [start, end] range
  uniformly across the words in that segment. Per-token timing via
  `get_token_data(i, j).t0/t1` would be substantially more accurate.
  Deferred because the per-token API has shifted between whisper-rs
  versions and I wanted foundation work to not introduce build-time
  risk. Pairs with the vendoring investigation.

- ✅ **Reset cadence per speaker** — landed v0.1.368.
  `speakers::reset_cadence_for(name)` zeros the Cadence EMA +
  persists. Backend `reset_cadence_for(name: QString) -> bool`
  + `current_active_speaker() -> QString` qt_methods. New
  `Intent::ResetCadence(String)` variant with parser covering
  self phrasings ("reset my cadence"), named-for phrasings
  ("reset cadence for Bob"), and possessive phrasings
  ("reset Bob's cadence") — with pronoun rejection so "reset
  her cadence" doesn't extract "her" as a name. Empty-name
  intent resolves via `current_active_speaker()`. QML dispatch
  emits toast confirmation. 10 unit tests in
  `reset_cadence_tests` module.

- [ ] **M — Cafe mode toggle.** User report: in noisy cafe settings,
  the VAD threshold rises with the noise floor, so your quiet
  whisper sits under the threshold and never triggers capture.
  The wake-word RMS threshold (`noise * 1.4`) does the same.
  Meanwhile, TonalProfile uses a fixed similarity (~0.65) without
  awareness of speaking style — whispered speech has flatter pitch
  contour and more breath, may score ~0.55 against your normal-voice
  profile and fail to identify. A "cafe mode" toggle should:
  1. Fix VAD threshold to a low absolute value (don't track noise floor up)
  2. Disable wake word (require tap-to-talk)
  3. Tighten OTHER-speaker similarity threshold (stricter rejection)
  4. Loosen YOUR speaker similarity threshold (looser self-accept)
  5. Optionally register a whisper-mode fingerprint via TonalProfile
     style_id=1 so your quiet voice has its own enrollment.
  Hard whitelist for command dispatch already works for the
  ignoring-others part — the gap is getting captured at all when you're
  quiet. Without this toggle, tap-to-talk is the workable fallback.

- [ ] **M — Environment weight / noise bias (sibling to cafe mode
  above).** User idea v0.1.149: instead of (or alongside) the
  discrete cafe-mode toggle, treat noise environment as a continuous
  bias that adjusts fingerprint scoring. In high-bias settings, all
  fingerprints get penalized in their scoring; the bar for trusting a
  match rises; ambiguous matches get demoted to Unknown rather than
  being assigned to a drift-prone profile. Speakers whose scores stay
  low for a while are relied on less often or temporarily removed
  from the active set. Directly reduces the speaker-lockout
  symptoms (filed elsewhere) because the system stops aggressively
  assigning drift-prone matches in conditions where it's likely
  wrong.

  Auto-trigger via WiFi SSID heuristics — SSIDs matching common
  venue patterns ("Cafe*", "*Starbucks*", "Public", "*-Guest",
  "WiFi-Free", venue chain names) raise the bias automatically;
  known-trusted SSIDs (home, office) keep it low. User-overridable
  via a manual slider in Settings, plus per-SSID memory so the
  system learns "this network = quiet" or "this network = noisy"
  from explicit corrections.

  SSID reading: **zbus** is the known-good path on Linux. Talk to
  NetworkManager over D-Bus (org.freedesktop.NetworkManager,
  ActiveConnection → Devices → Specific.Wireless.ActiveAccessPoint
  → AccessPoint.Ssid). Async, no shell-out, available on every
  systemd/NM distro the user is likely to be on (Fedora ✓). Also
  gives us a property-change signal so we can react to network
  changes instead of polling. Fallback path for non-NM systems can
  parse `iw dev <interface> link` output; defer until someone
  reports needing it.

  Cross-reference: the discrete cafe mode toggle above is v1; this
  continuous-bias version is v2. Build the toggle first; ship the
  continuous version once the toggle teaches us what the bias
  should actually DO (i.e. which scoring knobs are the right ones
  to move continuously vs. snap to a preset).

- [ ] **L — SpeechServices abstraction with typed staging.** User
  idea v0.1.149: the voice pipeline currently has many pathways
  (VAD, wake-listen, push-to-talk, per-speaker preview, whisper
  transcribe, intent parse, speaker-update, fingerprint
  augmentation, confusion-table learning) that read and write
  shared state (recent_utterance, current_active_speaker,
  fingerprint registry, confusion table, ...) without a documented
  ordering. Each new feature adds another implicit dependency;
  each developer's mental model drifts from the actual sequence the
  code executes. Symptoms: regressions when one path is touched,
  ordering races when bg pools complete out of expected sequence,
  hard-to-explain behavior in journals when multiple paths are
  in flight at once.

  Proposed: a SpeechServices module exposing typed stages in a
  fixed order, each owning a single piece of state:

      Raw audio chunk
        → VadStage           (emits Voiced | Silent | Burst)
        → WakeStage          (emits Idle | WakeArmed | Listening)
        → IdentifyStage      (emits SpeakerHypothesis + confidence)
        → TranscribeStage    (emits Transcript + timing)
        → ParseStage         (emits Intent + ranked candidates)
        → DispatchStage      (executes; emits IntentResult)
        → LearnStage         (updates fingerprints, confusion table,
                              cadence — gated on confirmed-good
                              outcomes)

  Each stage takes typed input, emits typed output, and is the
  ONLY place that mutates its associated state. State read by
  multiple stages (e.g. current speaker) is published by exactly
  one stage and read-only for the rest. Inter-stage queues are
  typed channels (Channel<VoicedChunk>, Channel<WakeEvent>, ...).

  This also gives the voice-drift / lockout fix a natural home: a
  ReAnchor sub-stage inside LearnStage that periodically validates
  the registered fingerprint against confirmed matches and updates
  conservatively, gated so it ONLY runs when the speaker has been
  confirmed (via floor-claim or unambiguous high-confidence match)
  recently. Without this abstraction, re-anchor would be yet
  another global mutator competing with the existing paths — a
  recipe for new regressions.

  Implementation cost: high. Substantial refactor across
  `src/voice_*`, `src/speakers.rs`, and the main.rs Backend
  fields. Right time is BEFORE adding more pathways, not after
  — the longer we wait the more migration cost accumulates. But
  this is L-sized; don't pick it up casually. Target a dedicated
  block of time where no urgent user-visible work is queued.

  Filed sub-task once started: write a docs/voice-pipeline.md
  that diagrams the stages and reads naturally as the source of
  truth for new contributors. The diagram itself is half the
  benefit of doing the refactor — even the documentation work
  before code touches anything would catch real ordering bugs.

- [x] ~~**S — Deep mute mode**~~ — landed v0.1.133.

- [ ] **M — Fingerprint polarity Phase B: augmentation worker.**
  Schema landed v0.1.132. Now: a background task watches the whisper
  pool for idle queue (empty >2s). Pops the most-recent confirmed
  utterance. Mixes 3 noise levels (-6, -12, -18 dB SNR) from
  audio_profile noise samples. Runs each augmented variant through
  whisper. Classifies:
  - Whisper says original transcript → positive for original label
  - Whisper says something different recognizable as English → negative
    for original, candidate positive for the new transcript
  - Whisper says garbage/empty → drop
  Atomic gate: worker pauses INSTANTLY when real user audio arrives.
  Cap: max 3 augmentations per source utterance, tracked per source.

- [x] ~~**S — Manual "wrong" feedback into negatives DB**~~ — landed
  v0.1.133. Each candidate row has a ✕ button; tap → audio stored as
  negative for that label, toast confirms. Effect immediate on next match.

- [ ] **S — Calibrate the negative-suppression penalty.** v0.1.132
  uses a fixed 1.4× multiplier when a negative is closer than the
  best positive. Should be empirically tuned. Once Phase B is
  generating real negatives, run a small A/B over test utterances
  and adjust.

- [x] **XS — Surface negative-feedback effect.** **Landed
  v0.1.290.** Toast text changed from "Logged X as wrong
  match" to "Logged — next time, similar audio won't match
  X". Calls out the forward effect explicitly so users see
  their feedback is doing work even before the next utterance
  proves it.

- [x] **M — Skip per-speaker whisper streaming when only one speaker
  is active.** **Landed v0.1.288.** The voice preview thread's
  per-speaker gate, previously `!speakers::is_empty()`
  (count > 0), now requires `speakers::count() > 1`.
  Per-speaker classification, buffer routing, and pool
  submission ALL skip when there's only 0 or 1 registered
  speaker. The cumulative streaming Whisper continues; the
  foreground voice_listen_dual is unchanged and remains the
  authoritative transcript. Mobile CPU recovery expected:
  ~3-5ms saved per 250ms tick plus an entire whisper_pool
  submission per utterance avoided.

- [ ] **M — SIMD audio pipeline (with Phase B).** The right time to
  introduce SIMD is when spectral subtraction + FFT-based noise
  augmentation land in Phase B. Those operations (frame-wise
  multiply-accumulate over band coefficients, complex magnitude
  computation) vectorize naturally. Use the `wide` crate for
  portability on stable Rust, or aarch64 NEON intrinsics via
  `std::arch` for max control on the user's Asahi setup.
  Retrofitting current code (DTW, MFCC, RMS) for SIMD yields
  ~7ms saved per ~1000ms utterance — not perceptible. Wait for
  the right hot path.

### Cafe-test findings (v0.1.169, observed v0.1.170)

User tested v0.1.169 in a noisy cafe. Tried re-registering own
voice and enrolling a new female-voice speaker. Both failed
("not working well"). Reading the captured log surfaces several
distinct issues — these are separate bugs, not one tuning knob:

1. **Pitch hint may be in wrong units.** Log lines like
   `[phonetic] using pitch hint 1798Hz (bin 35) instead of
   query-derived 1207Hz (×1)` — 1798Hz is far above any human
   fundamental (typical female F0: 165-255Hz, male: 85-180Hz).
   Two hypotheses:
     - "Hz" is a label error and the number is actually a bin
       index in some FFT scheme. Check whether the stored
       fingerprint stashes bin or Hz, and whether the unit got
       swapped at some point.
     - The hint is stored from a high harmonic or formant peak
       (e.g. F2 around 1500-2000Hz for some vowels), not from
       the fundamental.
   Either way, 1798Hz "overriding" a query-derived 1207Hz on
   every chunk is suspicious — should usually be CLOSE to the
   query's own pitch on the same speaker.

2. **Acoustic match collapses to 0.000 in noise.** Log shows
   most bursts come back `acoustic=0.000 (noise, not a wake-or-
   AI phrase)`. The acoustic scorer rejects so aggressively in
   noise that the wake-word never gets matched. Whisper still
   transcribes when forced ("but I think their first couple
   might have been a little", "lengthening the cycle now"),
   showing that the audio CONTAINS speech — just not from the
   target speaker, or the spectral match is too brittle to
   noise floor changes. Filed under noise-robust matching: see
   "spectral subtraction + FFT-based noise augmentation"
   (Phase B above) — this is the use case that motivates it.

3. **Speaker_match_score drift.** Across the session, "Me"
   scored 0.743, then 0.579, then 0.638. The threshold for
   "identified" appears generous enough that "Me" gets assigned
   to bursts that are *not* the user — and conversely, when the
   user IS speaking, the match score drops. Tied to (1) and
   (2): if the pitch hint or noise floor poisons the spectral
   compare, both directions of error get worse. Possible patch
   independent of fixing (1)/(2): tighten the threshold and
   require multiple consecutive matches before promotion.

4. **Wake-word re-registration overwrote the OLD wake-word.**
   User registered "hey, steering" (whisper misheard "hey
   apple") and the previous "hey apple" enrollment was
   replaced. Both phrases should be stored simultaneously when
   the user enrolls multiple. Filed under wake-word multi-
   phrase support.

5. **The female-voice enrollment failed to even produce a
   labeled speaker.** Logs show only "Me" — no new identified
   speaker entry. The enrollment flow needs to succeed
   independent of acoustic similarity to the existing user;
   currently it seems to be gated by the same matching that's
   broken in noise.

Filed items (each can ship independently):
  - [ ] **M — Pitch-hint unit audit.** Trace where the hint
    is stored, in what units, and reconcile vs query-derived
    pitch. If stored as a bin index, label it as such and
    compare in the same space.
  - [ ] **M — Threshold tightening + multi-frame confirmation.**
    Require k-of-n consecutive bursts to match before promoting
    "identified speaker = X." Reduces drift in noisy bursts.
  - [ ] **M — Wake-word multi-phrase storage.** Allow more than
    one enrolled phrase active simultaneously.
  - [ ] **M — Enrollment-gating audit.** Confirm that enrolling
    a new speaker isn't blocked by acoustic mismatch with
    existing speakers — fresh enrollment should produce a
    fresh slot regardless of how it scores against everyone
    else.

Won't address any of these without a quieter-environment
profiling pass first. Cafe testing surfaced the symptoms;
a controlled test (user in quiet room, female voice in quiet
room, both with intentional noise overlay) is the right way
to isolate which fix lands what improvement.

## Pending — significant features

### LLM path swap-protection + perf (v0.1.164 audit findings)

The v0.1.164 LLM path audit identified swap-exposure and perf
items the audit-and-fix-it-all-in-one-ship was too big for.
Filed for incremental follow-up:

- [ ] **M — `anthropic_api_key` swap-protection.**
  Currently the key is `QString` on Backend → `String` clone in
  `llm_query_async` for the worker thread → another `String` in
  the request builder. Three plaintext copies, all heap-
  allocated, all can swap. Fix: store the key in
  `Arc<Mutex<SecretBytes>>` on the Backend; worker thread takes
  an Arc clone and borrows the locked bytes when building the
  request. The Backend's `set_anthropic_api_key` method copies
  into the SecretBytes on write; the QString property mirrors
  for QML visibility but is wiped on app close.

- [ ] **M — `build_terminal_context` SecretBytes-backed.**
  Currently uses `format!()` and `push_str` against a regular
  `String`. Refactor to push into a SecretBytes buffer. The
  cleartext context (cwd, recent commands, summaries) lives in
  memory for the duration of an LLM call and can swap.

- [ ] **M — Aho-Corasick marker dictionary.**
  Current implementation: each line lowercased into a new String,
  then `.contains()` against each of ~60 markers. With Aho-
  Corasick, build the matcher once at startup, single-pass scan
  per line, no per-line allocation. Sub-ms gain per call but
  consistent.

- [ ] **L — End-to-end SecretBytes for LLM request body.**
  The JSON request body assembled in `claude_query` is a
  `String` that ureq copies into its socket write buffer.
  Custom `Write` impl into ureq's stream could collapse two
  allocations. Marginal perf gain (~1ms); main value is
  reducing the swap-exposure window for the redacted prompt.

- [ ] **M — Image inputs in llm_provider.** v0.1.172 user
  question: "ask AI (image models do they work over ollama?)"
  — confirmed yes. Ollama supports multimodal models via
  the `"images"` array on chat messages (gemma3, llama4,
  llava, llama3.2-vision). Anthropic's API also supports
  image inputs via the `image` content block type.

  Extend `llm_provider.rs` with a multimodal request shape:
    - New request type: `LlmRequest::Multimodal { text:
      String, images: Vec<ImageInput> }` where `ImageInput`
      is `{ bytes: Vec<u8>, media_type: String }`.
    - Anthropic encoder: base64-encode bytes, emit
      `{"type":"image","source":{"type":"base64",
      "media_type":"image/jpeg","data":"..."}}` content block.
    - Ollama encoder: base64-encode bytes, append to
      `"images"` array on the message.
    - Provider abstraction stays uniform; only the encoder
      differs.

  Prerequisite for:
    - "Ask AI" action in tap-hold per-image menu
    - Future "ask AI about screenshot" feature
    - Future "what does this diagram mean" voice command

  Model sizing realities:
    - On the Asahi MacBook: gemma3 (E4B, ~6-8GB VRAM)
      runs locally via ollama. Practical default.
    - On the phone: too big to run locally. Phone has to
      talk to laptop-hosted ollama, or to Anthropic.
      Plumbing has to handle "configured LLM endpoint
      varies by device" — already true today; just makes
      sure image bytes flow through correctly.
    - Anthropic's vision pricing: same as text input,
      with images counted by token-equivalents. Cheap
      enough to use casually.

  Privacy:
    - Tap-hold action menu's "Ask AI" sends one image at
      a time on explicit user gesture. No automatic
      uploads of page images.
    - For kid profiles: this is exactly the kind of action
      that the soft-guidance system (filed elsewhere)
      should prompt on. "want to ask AI about this image?"
      with parent-authored guidance text.

### Whole-app density mode (was "Braces Compact/Comfort")

User feedback v0.1.163: "braces comfort and compact was meant to
eventually be a whole-app setting". The `root.braceCompact: bool`
flag was sized as a per-feature setting but the user's intent was
always for it to govern the whole UI's density.

- [ ] **M — Promote `braceCompact` to `app.density: enum`.**
  Three modes probably: `Compact`, `Comfort` (default), `Spacious`.
  Drives:
    - Toolbar button sizes (currently hardcoded `width: 36;
      height: 36` etc.)
    - Pill paddings + font sizes
    - Settings drawer typography
    - Brace popup item size (current consumer)
    - Drawer / Popup paddings
  Touching dozens of components — file as M with the understanding
  that it gets done incrementally: add the property + the
  singleton/style object first, migrate one component family per
  ship.

- [ ] **L — Density-aware sizing singleton.**
  User v0.1.163 framing: "maybe we can refactor to sized types
  like how css has calc-> rust can have CalcUiSize that can be
  optimized and bonuded and derive into for conversions".
  Counter-proposal (filed for discussion before implementing): do
  this in QML, not Rust. A `Sizing.qml` singleton (pragma Singleton)
  exposing `Sizing.fontSm(density) / fontMd / pillRadius /
  buttonH / drawerPadding` etc. The singleton reads `app.density`
  once and computes; components read named sizes. Avoids:
    - Hundreds of backend properties for sizes
    - Rust ↔ QML round-trip on every layout change
    - Rebuilding to change spacing
  The "calc with bounds" part: each accessor clamps to a sane
  min/max so a future "10pt is too small" edge case can't make
  the UI unreadable.
  If user wants this in Rust specifically, ask why — likely
  motivation is type-checking, but QML bindings catch most
  size-misuse at runtime already.

### Slide popup primitive (STAGE M — partially landed v0.1.142)

Conversation v0.1.140 confirmed the user wants a shared
hold-and-slide popup gesture for several uses. v0.1.142 landed the
primitive + 2 of the 3 planned applications (pipe menu, brace menu).
Phase 3 (copy/paste menu) and drag-forward for braces are filed
below for a follow-up session per the user's "Option A + file the
difference" framing.

- [x] ~~**M — Slide popup primitive + pipe + brace menus.**~~ Landed
  v0.1.142. `qml/SlidePopup.qml` is the reusable component.
  Trigger: drag-click starting on host button (host calls
  `popup.beginFrom(host, x, y)` from its onPressed). Touch/mouse
  unified via PointHandler reparented to root on activation.
  Fade-in: 1000ms base − dragSpeed (clamped 80..1000ms) so fast
  drags get near-instant popup and slow drags see the animation
  as reward. Border grows from 2px → 4px when pointer crosses
  the popup's vertical bounds; hover freezes past that edge.
  Optional ±20px deadband emits open/both/close variants for the
  brace case. `pipeMenuModel` is 3×2 (|, &, ;, <, >, >>); pipe
  button replaces the standalone `|` chip that used to live in
  modRow. `braceMenuModel` is 1×6 with `itemSize: 96` to give
  the deadband zones comfortable thumb-target width. Brace button
  visible only when `backend.is_editor_active()` returns true
  (polled at 400ms from `_editorPoll`, same pattern as `_pagerPoll`).
  `is_editor_active` is a new Rust qt_method recognising `hx`,
  `helix`, `vim`, `nvim`, `nano`, `micro`, `emacs`.

- [ ] **S — Slide popup phase 3: copy/paste menu.** Deferred from
  STAGE M per user's "Option A" framing. The user's actual point
  in conversation was narrower than my earlier write-up: the
  buttons in the SSH side drawer are dead/unclear and need wiring
  up. So this is not "build a new copy menu" — it's "audit the
  drawer buttons, wire the missing ones, optionally add a slide
  popup if there are >3 actions that benefit from gesture access".
  Reuse `SlidePopup.qml` if a menu pattern is warranted.

- [ ] **M — Linear drag-forward for brace close-position.** Once
  the brace pair is emitted (`[]` with cursor between), CONTINUING
  to drag in the same direction emits arrow-right keystrokes to
  push the closing brace forward through the buffer. Reverse drag
  emits arrow-left to pull it back. Drag pixels → keystrokes via
  `termScreen.charW` (1 cell = 1 keystroke). Ghost `]` rendered
  at predicted position as visual feedback — per user spec,
  smoothed overlay with desaturation or negative (inverted)
  rendering rather than plain transparent. Row-change safeguard:
  read terminal cursor row from PTY state; if row changes
  unexpectedly (auto-indent ate our arrow-right), abort. Hold
  buffer up to 1000ms then fade out with bottom-screen toast
  "brace insert failed" (voice context only; silent for mouse/tap).
  v0.1.142 has `_braceInsertActive` flag and 1000ms guard timer
  already in place — the drag-forward listener and ghost char
  rendering still need building. Requires editor in insert mode
  (don't auto-toggle). Mouse/tap input blocked during the insert
  window (already wired in v0.1.142 via the `if (root._braceInsertActive) return`
  guards on the pipe/brace button onPressed handlers — extend
  this pattern to other input surfaces if needed).

- [x] ~~**S — Slide popup: variant hint visual polish.**~~ Partially
  landed v0.1.154. Replaced `(o)/(b)/(c)` text with directional
  arrows: `◀` for open (drag left), `▶` for close (drag right),
  nothing for "both" (center, the natural default). Still filed
  as a future polish: colored side-stripe on the option Rectangle
  reinforcing which half the pointer is in (cell highlight
  half-tinted left/right). The arrow alone may be enough; revisit
  after real-device test.

- [x] ~~**S — Slide popup: configurable popup position.**~~ Obsolete
  v0.1.154. The v0.1.142 filing was based on "above host with
  fallback below" — the original positioning. v0.1.144 replaced
  that with press-point-centered + clamp-to-bounds. If the host
  is at top of window, the press is at top, the popup wants to
  center on the press but the upward overflow clamps it down —
  net effect: popup ends up BELOW the host naturally. No
  `preferredSide` needed. Closing as obsolete; revisit if a
  specific host needs side-specific positioning in the future.

- [x] ~~**S — Brace popup: comfort-vs-compact setting.**~~ Landed
  v0.1.155. Settings pill toggles `root.braceCompact`. Compact =
  deadbandPx 14, itemSize 80 (side zones 26px each). Comfort =
  deadbandPx 28, itemSize 120 (side zones 32px each). Default
  comfort. Session-only persistence — values reset on restart;
  if real usage shows a strong preference, plumb through
  backend.save_settings in a follow-up.

- [ ] **S — Slide popup alternative layouts to test.** User
  mentioned wanting to test "stacked that separates the brackets
  NW and SE as the drag point approaches their group center with
  a slight fade." Possibly emulatable as a 2-column grid with
  opacity-shaped delegates. Real-device experiment session, not
  a planned ship.

### GestureHost reuse — opportunities to wire next

v0.1.143 attempted to extract the input-owning pattern as a separate
component (`qml/GestureHost.qml`). v0.1.144 dropped the abstraction
and inlined the same pattern directly in main.qml for the pipe and
brace buttons because the abstraction had a bug that lost mouse
presses. The primitive idea is still good — re-extract once the
inline pattern is confirmed working on real hardware AND we have a
second user that would exercise it.

- [x] ~~**S — Re-extract GestureHost once inline pattern is proven.**~~
  Landed v0.1.148. `qml/GestureHost.qml` wraps the v0.1.144+
  inline 4-handler pattern; the three existing call sites
  (toolbar pipe, toolbar brace, block-row `⋯`) now use it,
  shedding ~67 lines of boilerplate. Properties: popup,
  gestureContext, preventStealing, hoverEnabled, containsMouse,
  pressed. Filed-but-blocked reuse items below are now unblocked.
- [x] ~~**S — Block header per-block menu.**~~ Landed v0.1.146 as
  the `⋯` button in the block-row delegate, with a 2×2 popup
  ({rerun, copy command, copy output, toggle summary}). Existing
  inline icons kept for one-tap access. Single `_blockRowPopup`
  instance serves all rows via `gestureContext: model.blockId`.
  Parented to `Overlay.overlay` so it renders above the
  blocksDrawer.
- [x] ~~**S — File picker row per-file menu.**~~ Landed v0.1.152.
  `⋯` button on each row in filePickerDrawer's column-mode delegate,
  same pattern as the block-row menu. 3x1 horizontal popup:
  📋 Copy path | 📂 Open | ↪ Insert path at cursor. "Open" is the
  center cell so tap-without-drag commits the existing navigation
  behavior. Insert path quotes the path if it contains shell-
  special chars and closes the drawer after insertion (the user
  was navigating to find a file to use in a command — that's the
  moment they want).
  Single-pane mode (when useColumnPicker=false) doesn't have the
  menu yet — filed as a v2 follow-up. Larger menu shape (Share,
  Image preview, Reveal in terminal, Edit) also v2.
- [ ] **M — Voice transcript word actions.** Per-word menus for
  {Correct phonetic, Ignore this word, Register fingerprint}.
  Blocked on the per-word data model existing in QML scope —
  currently word fingerprints are backend-only. Wire when
  exposing word objects to QML for any other reason.
- [ ] **S — Settings slider precision zones.** Different gesture
  model from the popup (the "arc-shape" item filed elsewhere)
  but conceptually the same pattern — host-owned drag with
  visual feedback. Worth checking if the input-ownership
  pattern generalizes enough or if it needs a sibling.

- [ ] **M — Tap-hold per-image action menu.** v0.1.172 user
  direction: "if user holds down on image area we can open
  up options and swipe to each of them." Long-press on any
  rendered image surfaces a radial/strip of action options;
  user swipes (without lifting) to the desired action and
  releases to commit.

  Initial action set:
    - **Save** — write image bytes to disk via file picker
      / downloads folder
    - **Bookmark / star** — add to a per-user collection
      (composes with future profile system)
    - **Ask AI** — send the image to the configured LLM
      provider with a "what is this?" prompt; show response
      in chat. Requires the M-sized "image-models-over-LLM-
      provider" filing (below) — blocked on that.

  Composability:
    - GestureHost ownership pattern (host watches all touches,
      delegates to focus owner) is what makes this clean —
      we don't have to add MouseAreas to every image.
    - Hooks into the QQuickAsyncImageProvider work (the
      provider knows the image's source URL, decoded
      dimensions, fetch metadata; the action menu queries
      it without re-fetching).
    - Per-user state (bookmarks live per identified speaker)
      composes with the voice profile system.

  Risk: long-press conflicts with browseFlick's existing
  press-to-scroll. Need a press-and-hold threshold (~500ms)
  distinct from press-then-drag (which is scroll).
  GestureHost already does this discrimination for some
  surfaces; reuse the same approach.

### v0.1.144 follow-ups

- [x] ~~**M — Selection region width on multi-line drag.**~~ Fixed
  v0.1.147. Root cause was a hardcoded `79` in the per-row
  delegate's `colEndIncl` calculation at the original selectionLayer
  (line ~1795 in v0.1.146). For middle rows of a multi-line
  selection, the highlight extended only to column 80 regardless of
  terminal width — the "fine slightly-wide is harmless" comment
  next to it was wrong on a wide laptop terminal. Replaced with a
  `_visibleCols` property derived from `termScreen.width / charW`,
  which gives the actual viewport column count. Data path was
  always correct (copy/paste returned the full text); only the
  visual overlay was wrong.
- [x] ~~**S — Bottom-screen toast widget.**~~ Already existed (since
  before this changelog tracked things — used for "Copied X chars"
  feedback, workflow completion, etc.). v0.1.151 extended it with
  severity variants (info / success / warning / error), wired
  clipboard-copy success/failure feedback to it (both the inline
  ▶ 📋 📥 buttons AND the gesture-menu dispatch), and surfaced
  the v0.1.150 startup probe via a delayed toast warning if no
  clipboard tool is available.
- [x] ~~**S — Verify wl-copy / xclip presence at startup.**~~ Landed
  v0.1.150. `clipboard-probe` runs at main() startup, PATH-walks for
  wl-copy/wl-paste/xclip/xsel/pbcopy/pbpaste, logs which are present,
  and prints a loud `⚠ COPY WILL FAIL` warning if none of the
  copy-capable set is installed. Same for paste. Surfaces the silent
  "copy does nothing" failure at startup instead of on first button
  press.

### v0.1.146 follow-ups

### v0.1.148 follow-ups

### v0.1.152 follow-ups

- [x] ~~**S — File picker single-pane mode also gets ⋯ menu.**~~ Landed
  v0.1.154. Same GestureHost shape as the column-mode delegate,
  same shared `_filePickerRowPopup` instance. gestureContext
  always points at the last column since single-pane only shows
  one. Same hit-test trick: tap-to-open MouseArea declared FIRST
  so the later-declared RowLayout (containing GestureHost) renders
  on top at the ⋯ region.

- [ ] **S — File picker per-row menu: larger action set.** Current
  v1 is 3 actions in a 3×1 horizontal layout. v2 should expand to
  a 3×2 grid with type-aware action visibility:
    - Directories: Open, Copy path, Reveal in terminal (cd), Share
    - Files: Open, Copy path, Edit, Insert path, Share
    - Image files: + Preview
    - Symlinks: + Resolve, Show target
  Implementation needs the popup model to be dynamic per row (the
  GestureHost passes the entry kind in gestureContext already; the
  popup just needs to filter the model based on it). The current
  shared-popup pattern works fine but the model is currently
  static — would need either per-host popup instances OR a
  dynamic-model variant of SlidePopup.

- [x] ~~**S — Regex-aware brace counter for QML lint.**~~ Landed
  v0.1.157 as `scripts/qml_lint.py`. The counter is
  `strip_strings_comments_regex()` — strips strings, comments,
  AND regex literals before counting `{}`, `()`, `[]`. Regex
  detection uses a JS-context heuristic (`/` is regex start if
  preceded by operator/punctuation, division otherwise). The
  full lint script bundles this with the unguarded-backend
  check (see related filing landed v0.1.157).

### v0.1.153 follow-ups

- [x] **S — Voice intent toast: opt-out toggle.** **Landed
  v0.1.167.** Backend `voice_intent_toast` qt_property
  (default true) gates the v0.1.153 dispatched-intent toast.
  Settings UI surface in QML at the speakers-row neighborhood
  (around qml/main.qml:14936) reads/writes the property. Default
  ON preserves the safety feature; users can opt out in
  Settings when comfortable.

- [ ] **M — Speaker lockout from own profile after extended
  background-noise speaking.** User report v0.1.148:
  "speakers can get locked out of their own profiles after a
  lot of background noise speaking because sometimes both is
  user talking and is noise background can be true... these
  patterns of long transactions are what it tends to look
  like". The wake-listen journal shows the pattern clearly:
  bursts being attributed to whisper hallucinations ("wah",
  "Hello. Hi", "el y boto") instead of being recognized as
  the registered user, even though the user is the source.
  User noted "actually wait it looks like this was user
  drift" — the registered fingerprint may be drifting from
  the actual voice (which is itself shifting under fatigue
  or different posture), and rather than re-matching it as
  a same-user-different-style case, the system locks the
  profile and treats subsequent speech as unknown.
  Cross-reference with the L item "User merging/diverging
  when same voice shifts style" under multi-user — this is
  the same root cause manifesting from the wake-listen
  side. Possible mitigations: a confidence-drift hysteresis
  that requires N consecutive misses before locking; an
  explicit unlock-via-floor-claim path; periodic re-anchor
  of the registered fingerprint against confirmed matches.

- [ ] **S — File picker unnamed folders issue (still).** User
  report v0.1.148: "file picker still has the unnamed folders
  issue". This was tackled defensively in v0.1.141 with a
  fallback lookup + diagnostic logging in the "_appendColumn"
  / "header" paths. The fallback was supposed to make the
  visible bug invisible while we collected diagnostic data
  to find root cause. If the user is STILL seeing unnamed
  folders, either (a) the fallback isn't firing (binding
  path or scope mismatch — check the logs from a fresh run),
  or (b) the data being passed is genuinely empty at the
  source (column_data.path is empty string, not undefined,
  so the fallback condition doesn't trigger). Next session
  on this: read the [picker._appendColumn] and [picker.header]
  log lines from a real reproduction, then fix accordingly.
  Don't bandage again — root-cause it this time.

- [ ] **M — Scroll in helix shows scroll characters but no
  scrolling.** User report v0.1.148. When using helix's
  built-in scrolling (Ctrl-d / Ctrl-u, j/k at edges) the
  ASCII scroll indicator updates in helix's UI but the
  actual terminal content doesn't move. Likely PTY-side:
  helix is sending alt-screen output that's being captured
  but not re-rendered, OR the terminal is suppressing
  redraws for some reason in alt-screen mode. Cross-reference
  with the "smooth-scroll passthrough for spammed pager keys"
  L item — this is a different symptom but may share root
  cause (alt-screen handling of repeat output). Look at
  pty_term's alt-screen path and whether grid mutations
  in that mode fire the proper signals to QML.

- [x] ~~**S — Brace menu host visual sizing.**~~ Landed v0.1.150.
  Shrunk from 38×32 to 32×28; font dropped from 13px to 12px;
  radius from 8 to 7. The "{}" text fills more horizontal space
  than "|⋯" so the same dimensions gave heavier visual weight;
  narrower host + smaller text restores the per-row balance.

- [x] ~~**S — Wake-listen hallucinations during ambient chatter
  (re-investigation).**~~ Landed v0.1.153. The "long burst with low
  acoustic — running whisper anyway for ai-wake check" bypass in
  `src/voice.rs` now rejects bursts with acoustic < 0.005 (which
  the impl returns as exactly 0.0 when phonetic::top_matches doesn't
  even consider __wake_word__ as a candidate). These are the
  pure-noise cases that produced "wah", "Hello. Hi", "Absolutely
  right" hallucinations in v0.1.148. Long bursts with low-but-
  nonzero acoustic still pass through (legitimate "ask ai"
  phrases with different phonetics).

- [x] ~~**S — Regex-aware brace counter for QML lint.**~~ Landed
  v0.1.157 as part of `scripts/qml_lint.py`. The regex-aware
  brace counter is `strip_strings_comments_regex()` — strips
  string literals, single-line/block comments, AND JS regex
  literals before counting `{}`, `()`, `[]`. Regex detection
  uses a JS-context heuristic (`/` is regex start if preceded
  by an operator or punctuation; division otherwise).

- [x] ~~**S — Segfault on null backend in scene graph bindings.**~~
  Lint landed v0.1.157 as `scripts/qml_lint.py` —
  `check_unguarded_backend()` flags `backend.<method>()` calls
  in QML binding context (property RHS, not handler bodies)
  without a recognized null-guard within 6 lines. False
  positive rate is moderate (~50%) on existing code due to
  patterns the heuristic can't follow: transitive guards via
  derived properties, object-literal property keys that look
  like bindings, function bodies inside Connections. Suppress
  with `// LINT-OK: <reason>` comments inline.
  Caught two real concerns: dolphin_favorites_list called
  without backend guard (line 10118), term_block_output binding
  guarded by `expanded` but not by `backend` (line 10696). Both
  fixed in this ship.

- [ ] **M — Selection overlay: alternative "merged box + edge tickmarks"
  visual.** User idea, filed v0.1.146: instead of the current
  per-line full rectangle overlay (each selected line gets its own
  highlight rect spanning the selected columns), render a SINGLE
  bounding box that wraps all selected lines, with tickmarks along
  its edges to indicate the actual start/end columns. Cleaner
  visually for large selections; the per-line rects can look noisy
  on multi-line drags. Should be a Settings toggle so users can
  pick: "Per-line rectangles" (current) vs "Merged bounding box
  with tickmarks" (new). Implementation: selection overlay
  rendering is in termScreen / pty_term area; today it iterates
  per-row producing one Rectangle per selected row. New mode
  computes the bounding rect (min/max row, min/max col) plus
  tickmark positions at the actual boundary rows. Cross-reference
  with the multi-line width bug below — the two interact since
  both touch the overlay geometry.

- [ ] **S — Per-block summary toggle.** v0.1.146's `⋯` menu's
  `✨ Summary` action toggles the GLOBAL log_summary_set_enabled.
  Per-block would need a HashMap<block_id, bool> override in the
  Rust block tracker and a new qt_method. Useful if AI summaries
  become expensive (per-block opt-in instead of all-or-nothing).
- [x] ~~**S — `⋯` button visual differentiation.**~~ Landed v0.1.150.
  +4px wider (26→30), and a subtle border in the resting state
  (1px, 35%-opacity foreground muted color). On hover the border
  fades into the hover-color fill so it visually matches the
  sibling action buttons during interaction. Hints "menu trigger"
  vs the borderless one-tap actions next door without being
  obtrusive.
- [ ] **S — Decide: replace inline ▶ 📋 📥 with `⋯` popup only?**
  v0.1.146 keeps both side by side so the user can compare. After
  testing, decide whether the popup obsoletes the inline icons
  (cleaner row) or if they should coexist (one-tap for power
  users, popup for thumbs).

### Big interactions

- [ ] **L — Joystick replacing 4 arrow buttons.** User confirmed
  no phantom cursor for v1 — just acceleration → repeat count
  mapping. Drag direction picks arrow; drag distance from center
  determines number of repeats; acceleration curve tunes feel.
  Real-device iteration needed. v0.1.140 arrow auto-repeat
  serves as the fallback while this is being designed.

- [ ] **L — Font zoom arc-shape gesture.** User sketch: from
  the font-size slider in Settings, drag horizontally to zoom +
  live update. Distance from drag-start = precision amplifier
  (further = finer control, closer = coarser). Arc shape (not
  pure V) means the precision feature is a range amplifier
  variable. Real new interaction pattern; deserves a design
  session before implementation.

### v0.1.141 testing — filed for follow-up

User reported these alongside confirming v0.1.141's fixes.

- [ ] **M — Three-tier scrolling sensitivity (global).** Touchpad
  and touch behave differently for the same intent: touch is more
  1:1 (finger moves N pixels → content moves N pixels), touchpad
  multiplies (small physical motion → large content motion). Goal:
  detect input source and apply per-source sensitivity multiplier.
  User sketch: touchpad ×1, touch ×0.1 (because the multiplier on
  touchpad is wanted, on touch it would feel wrong). Wheel/mouse
  is a third tier. Detection via Qt's `event.source` and
  `event.device.type` where available; falls back to global
  default when ambiguous. Apply to: terminal scrollback, browser
  scroll, lists in drawers. **Honest caveat:** the detection
  question is itself non-trivial — same physical touchpad can
  send synthesized vs native events depending on driver/Wayland
  state. May need a manual override toggle in Settings as
  fallback.

- [ ] **S — Tab switch from terminal tab to browser tab flickers
  and doesn't switch.** When user is on terminal tab and clicks
  the browser tab, the highlight briefly moves and then snaps
  back. Likely a Connections/binding that resets currentIndex on
  some signal. Suspect culprits: termKeyArea's `forceActiveFocus`
  being called when terminal tab becomes "active", a Component
  binding on currentIndex, or URL bar's focus snapping. Should
  reproduce reliably — test by repeatedly clicking the browser
  tab while terminal is active. Fix: trace which handler is
  resetting currentIndex via console.log breadcrumbs.

- [x] ~~**M — "Copy output" action on command-history blocks.**~~
  Already landed before this changelog tracked it — the 📥 icon
  on each row in v0.1.144 + the ⋯ menu option in v0.1.146 both
  copy block output. Marking complete; the filing was stale.

- [x] ~~**S — Existing blocks drawer buttons have no tap handlers
  or unclear purpose.**~~ Audited v0.1.156. The complaint was
  stale: all row-level buttons (▶📋📥 from v0.1.144, ⋯ from
  v0.1.146) have onClicked handlers and tooltips. Header had
  an unlabeled ↻ with no tooltip — v0.1.156 replaced with a
  labeled "↻ Refresh" pill with tooltip explaining it reloads
  from the backend. No truly dead buttons found.

- [x] ~~**S — Copy/paste/select-row should be in the auto-condensing
  toolbar.**~~ Landed v0.1.156 via height-and-opacity compression
  of the secondary row rather than moving the buttons into the
  main row. Reasoning: the right-side info displays (scrollback
  arrow, dimensions readout) are useful and shouldn't move;
  compressing the whole row is a smaller diff with less risk
  to existing layout. Row shrinks from 22px → 14px and fonts
  scale down (10→8) when `_touchRow._compressed` is true (same
  trigger as the main row's typing-collapse).

- [ ] **M — Selection rectangle visually too narrow in ssh on
  laptop.** Selection text COPIES correctly (data is right), but
  the visual overlay rectangle ends short of where the actual
  text tail is. Likely cause: per-row character-width assumption
  (`termScreen.charW`) is based on the local terminal's column
  count, but the ssh remote may be reporting/using a different
  effective width. Selection bbox math then uses the wrong
  char width per cell. Investigate: compare local `term_cols`
  vs the COLUMNS env var as seen in the remote, see if the
  mismatch correlates with the visual offset. Fix probably means
  reading the actual rendered cell width from the row data
  rather than the calibrated charW.

- [ ] **M — Line wrap on long commands introduces extra blank
  line, offsets subsequent cursor.** When a long command wraps
  visually (no actual newline in PTY, just terminal-driven wrap),
  the terminal appears to think the wrap consumed an extra line.
  Subsequent prompt or output renders one row too low. User's
  proposed workaround: after a non-pager command completes,
  fire synthetic `Ctrl+L` to clear+redraw, preserving scrollback
  buffer separately so the clear doesn't wipe history. **Honest
  concern:** workaround has UX side effects (visible flicker on
  every command, even short ones). The clean fix is finding the
  actual wrap-handling bug in the terminal emulator code (likely
  in `src/term/`). Try real fix first; fall back to documented
  workaround if root cause is elusive after one investigation
  session.

- [ ] **S — Picker sub-column shows count, not name.** v0.1.141
  added defensive fallback for the "undefined header" bug. User
  reports a related symptom: after navigating with the back-arrow
  at top-left of the picker, the sub-column displays a folder
  COUNT where the folder NAME should be. So modelData.path is
  probably holding `entries.length` (or similar numeric) instead
  of the path string. Strong signal that the bug is in
  `_appendColumn` push semantics or `_navigateIntoFromColumn`'s
  navigation logic, not just a binding refresh issue. Diagnostic
  logs added in v141 should reveal which. Next picker-touching
  session: read the logs first, fix root cause, remove the
  defensive fallback.

### Terminal interaction polish (v0.1.130 user feedback)

- ✅ **Scrollback search** — landed v0.1.349. Ctrl+F on plain
  shell opens a find bar above termScreen; Enter / F3 / arrow
  buttons navigate; Escape closes. Alt-screen passthrough
  preserved for vim/helix/less. Voice "find <term>" routes
  by active tab. See `docs/power-features.md` Phase 1.

### Scrollback search follow-ups (v0.1.349)

- ✅ **Highlight all matches at once** — landed v0.1.350. Repeater
  layer renders every match in the visible viewport at 25% alpha;
  the current match stays at 65% alpha so it visually dominates.
- ✅ **Case-sensitivity toggle** — landed v0.1.355 as the Aa
  button in the find bar. Active state borders in accent;
  re-runs search on toggle so the change is visible immediately.
- ✅ **Persistent search history** — landed v0.1.355. Up/Down
  arrows in the find field recall prior queries. Captured 20-
  deep, in-memory only (resets on app restart). Pre-history
  text restored when walking past the newest entry. Pushed on
  Enter (commit) and on close (capture-the-final).
- [ ] **M — Regex mode toggle.** Skipped in v0.1.355 because the
  `regex` crate isn't currently a dep and adding it for one
  feature is over-scoped. Decide: add the dep (cost ~5-10s of
  compile time, well-trusted crate), or use a hand-rolled glob-
  style matcher (limited but cheap)?
- ✅ **Find in code (filesystem ripgrep mode)** — landed
  v0.1.361. New `src/fs_find.rs` module (worker-thread search,
  AtomicBool IN_FLIGHT, result cap 100, MAX_LINE_LEN 200).
  Backend probes for `rg` at module init; falls back to
  `grep -rIn -F` when absent so power users without ripgrep
  installed still get a functional (slower) path. Find bar
  gains a mode toggle (scroll glyph ↔ folder glyph) next to
  Aa. In files mode: Up/Down navigates results, Enter commits
  the selected path (shell-quoted) into the PTY via pty_write,
  Esc closes. Results render in a panel below the bar with
  hover-to-select and click-to-commit. 7 unit tests pin the
  parse_match_line edge cases.

  Follow-ups filed:
  - **S — Regex mode for files search.** Today the grep
    fallback uses `-F` (fixed-string); ripgrep defaults to
    regex. Add a toggle so the user picks. Mostly UI work,
    plus a `regex: bool` arg to fs_find_start.
  - ✅ **Glob filters for files search** — landed v0.1.373.
    Two parser forms accepted via `fs_find::parse_glob_query`:
    shorthand `*.rs:fn main` (looks for glob chars `*`/`?`/`[`
    before the colon) and explicit `glob:PATTERN:query` (no
    glob-char check — user knows what they're doing). When a
    glob is present, `rg --glob PATTERN` narrows the search;
    grep fallback uses `--include=PATTERN` (stripping leading
    `**/` since GNU grep doesn't recurse via that). Literal
    queries like `http://example.com` or `foo::bar` stay
    untouched. 11 unit tests in fs_find::tests.
  - **S — Open-in-editor on shift-Enter.** Today Enter pastes
    the path. Shift+Enter could invoke `$EDITOR <path>` +
    newline so the user doesn't have to compose the command.
    Needs $EDITOR detection + a confirmation flow for first
    use ("invoke vim with this file?").
  - **M — Block-tracker integration: clickable file:line in
    smart-tokens.** When the existing block-tracker renders
    output that LOOKS like ripgrep output (path:N:text), make
    the path:N segment a smart-token that opens the file. Same
    pattern as URL detection. Bigger ship; gates on block-
    tracker rendering changes.

### Long-running command notifications (v0.1.350)

Phase 1 power-feature: toast when a block finishes if user has
switched tabs or defocused the window. Threshold default 10s,
user-settable + persisted via settings.conf. See power-features.md.

- ✅ **Core notification path** — landed v0.1.350. Block tracker
  queues pending notifications on block close; main.rs drains
  in `pty_read_screen` and emits `long_command_finished`. QML
  Connections handler shows a toast when appropriate.
- ✅ **Settings UI for the threshold** — landed v0.1.369.
  New "Long-command notifications" section in the settings
  popup, just above "Token cost estimation". TextInput accepts
  0-3600 seconds; 0 disables. Shows a live "≈ Xm" hint to the
  right when the value is over a minute. Persists via existing
  save_settings on edit-finished.
- [x] ~~**M — Capture exit codes from prompt markers.**~~ Done
  v0.1.883. OSC 133 `D;<code>` shell-integration markers are now
  parsed in `pty_term.rs` (`parse_osc133_exit` + an `osc_dispatch`
  `b"133"` arm) and captured onto `TermGrid.osc133_exit`. Each pty
  tick, `pty_read_screen` drains it via `PtyTerminal::take_osc133_exit`
  (non-blocking `try_lock`) into `BlockTracker::note_exit_code`. On the
  next command submission the code is attached to the block that just
  finished and rides the `pending_notifications` tuple (now
  `(id, cmd, dur, Option<i32>)`) out to `long_command_finished`, which
  surfaces it as a real exit code instead of the hardcoded -1 — unlocking
  "Build failed (✗)" vs "Build done (✓)" toasts. Shells without OSC 133
  integration carry `None` → the legacy -1 (unknown), so behavior is
  unchanged for them. Tested end to end: OSC-133 parse table (D;0/D;1/
  D;130/bare-D/A-B-C/non-numeric), VTE-level capture onto the grid, and
  the BlockTracker attach-on-next-submit path. The QML/signal hop
  (drain → note → toast) compiles clean but its on-screen ✓/✗ wants a
  runtime confirm with a shell-integration-enabled shell.
  Original note follows.

  Currently
  the BlockTracker never sets `exit_code` (always `None`), so
  the notification's exit_code arg is always -1. Most modern
  shells emit OSC 133 prompt markers including the exit code
  of the previous command. Parse them in pty_term.rs and set
  via a new `BlockTracker::record_exit_code(code)` call from
  main.rs's prompt-marker dispatch. Unlocks "Build failed (✗)"
  vs "Build done (✓)" in the toast.
- [ ] **M — OS-level notifications when window fully unfocused.**
  Currently only shows the in-app toast. When `!root.active`
  AND the OS supports it, fire a libnotify (Linux) /
  NSUserNotification (macOS) so the user gets a real desktop
  notification. Toast still fires in-app for when they return.
- [ ] **S — Per-command notification overrides.** A small allow/
  deny list in settings: "always notify for cargo test even
  if quick" / "never notify for sleep". Pattern-matched
  against block.command on close. Optional; current behavior
  (single threshold) is enough for most uses.

### Keybinding audit + chord overlay (v0.1.353 — Phase 1 power-features)

User: "missing keybindings like arrowkeys and the multi-tab and
the minimize, maximize, plus others I probably don't know." This
ship fills the gaps plus adds a chord-help overlay so users can
discover what's available.

- ✅ **Keybinding audit + ? overlay** — landed v0.1.353. Added
  Ctrl+= / Ctrl+- / Ctrl+0 for font size, F11 fullscreen,
  Ctrl+T new browser tab, Ctrl+W close browser tab, Ctrl+,
  open settings, Ctrl+P open palette, F1 / Ctrl+/ / Ctrl+? open
  the chord overlay. Voice "show keybindings" / "show help"
  also opens it. The overlay groups chords by category and is
  registered as a single QML array (`root.chordRegistry`) for
  manual sync with the actual Shortcut elements.
- ✅ **Auto-sync overlay with Shortcut elements** — landed v0.1.354
  as a qml_lint check. New `check_chord_registry_sync` parses all
  Shortcut sequences and the chordRegistry array, warns when a
  sequence isn't documented. Override marker `// chord-registry-
  skip` on the same or preceding line suppresses individual
  warnings. Already caught a real divergence on landing: registry
  used `Alt+\u2190` (arrow glyph) while Shortcuts used `Alt+Left`
  — fixed by adding optional `display` field for prettier
  rendering while `keys` stays Qt-spelling for lint.
- ✅ **Per-context chord filtering** — landed v0.1.372. Chord
  registry entries gain an optional `context` field
  ("browser" / "terminal" / "any" or undefined). Toggle pill at
  the top of the chord overlay defaults to ON (showing only
  current-tab chords); tap to show all. Hard browser-only
  shortcuts (Ctrl+T, Ctrl+W, Ctrl+P, Alt+Left/Right, Ctrl+Shift+R)
  and hard terminal-only ones (Ctrl+C, Ctrl+D, Ctrl+Z,
  Ctrl+Shift+C, Ctrl+Shift+V) tagged. Dual-meaning (Ctrl+L,
  Ctrl+R/F5, Ctrl+F) and global (F11, Ctrl+,, font sizing, F1)
  stay untagged. AND-combines with search-box filter from
  v0.1.371.
- ✅ **Search box in the overlay** — landed v0.1.371. TextInput
  at top of the chord overlay filters by keys, display label,
  description, OR category (substring, case-insensitive).
  Categories with zero matching entries collapse. "No matches"
  state renders when nothing matches. Esc clears search before
  closing the overlay (one Esc to clear, second to close).
  Autofocus on open so power users can type immediately.
- [ ] **S — User-defined chord palette.** Already filed in
  power-features.md as the "Customizable chord palette" item.
  Once we have a per-user chord storage layer, the overlay
  shows user chords alongside built-ins.
- ✅ **Keyboard quick-jump (section variant)** — landed
  v0.1.384. Alt+N/E/T/V/W/H jumps to a category section in
  the chord overlay (N=Tabs & Navigation, E=Editing,
  T=Terminal, V=Voice & AI, W=Window, H=Help). Each section's
  delegate Column gets an `objectName` like
  `chordSection:Editing` so the jump function can find and
  scroll to it via the Flickable's `contentY`. Footer hint
  updated to `Esc to close · Alt+N/E/T/V/W/H jump to section`
  so the feature surfaces visually. Six `Shortcut` blocks
  guarded by `enabled: chordOverlay.opened` so the chords
  don't steal Alt+T (etc) from the rest of the app.

- [ ] **S — Keyboard quick-jump (row-fire variant).** The
  ORIGINAL spec of this feature was: press the shown key
  combination to highlight that row and preview its action,
  with a follow-up press to fire it. v0.1.384 shipped the
  category-section version instead (simpler, no separate
  preview-vs-fire state). The row-fire variant remains
  filed as a richer future feature — requires showing a
  highlighted-row state, listening for the actual chord
  rather than just letters, and a confirm-fire flow.

### Token accounting (v0.1.352)

Per-call token logging + spike-detection man page. User-entered
$/1M pricing fields. See `docs/changelog/0.1.1x.md` for the
shipped surface.

- ✅ **Core logging + man page** — landed v0.1.352. JSONL log at
  `~/.config/web2qml/token_log.jsonl`, two pricing fields in
  settings, `web2qml://stats/tokens` builtin URL renders the
  man page with per-op stats + top-5 spikes + estimated cost.
- ✅ **Op-tag refinement** — landed v0.1.365. `provider_query`
  and `claude_query` now take an `op_tag: &str` parameter that
  flows into the token-log event. Empty tag falls back to
  "claude_query" for back-compat. Three caller surfaces in
  main.rs updated with distinct tags:
  - `llm_query` (qt_method, sync) → `"llm_query"`
  - `llm_query_async` (qt_method, async) → `"llm_query_async"`
  - `llm_query_with_context` (terminal-state-enriched) →
    `"context_question"` when enrichment fires, or
    `"context_question_unenriched"` when send_terminal_context
    is off (the off-path is distinguishable in stats).

  Factored llm_query_async into a private
  `llm_query_async_tagged(prompt, op_tag)` helper so the
  context-question path can pass its own tag without
  re-implementing the worker plumbing. chat.rs left alone
  (already had a distinct `"chat"` tag).
- [ ] **S — OpenAI / Ollama instrumentation.** llm_provider's
  Ollama paths and chat's OpenAI path aren't instrumented.
  Local Ollama has no cost but the spike view still wants
  the data. OpenAI is comparable to Claude for cost.
- ✅ **Time-window filters in the man page** — landed v0.1.364.
  New `TimeWindow` enum (Session / Today / Last7d / AllTime) +
  `filter_by_window` helper. Page emits a markdown tab nav at
  the top: active window bolded as plain text, inactive ones
  as `[Label](web2qml://stats/tokens?window=slug)` links. URL
  handler in main.rs parses `?window=X` and threads through to
  `render_manpage`. Session start tracked via new
  `session_start_unix: u64` field on Backend (plain field —
  QML doesn't need direct access). `parse_event_ts` handles
  both the current unix-seconds-as-string format AND ISO 8601
  with fractional seconds, so the dual-format isn't a barrier
  if we switch later. 11 new unit tests cover the lot.

  Filed follow-ups:
  - **S — Local-tz refinement for "Today".** Current
    implementation uses UTC midnight; users in non-UTC
    timezones see "Today" boundaries shift in confusing ways.
    Either pull in chrono-tz (~adds 1 MB to binary) or
    hand-roll using $TZ + the system tzdata. Acceptable for v1
    because the SHIFT is small (most days you'd just see one
    extra event from yesterday's late-night work counted in
    Today, or vice versa).
  - **S — Persist session_start across crash/restart.** Today
    if web2qml crashes mid-session, the next launch resets the
    session window. Could write the boot timestamp to a sibling
    file (`session_start.txt`) and reuse on launch if it's <
    24h old. Mild quality-of-life improvement.
- ✅ **Reset button** — landed v0.1.365. New `reset_log()` /
  `reset_log_at(path)` in token_log.rs renames the JSONL to a
  sibling `.bak` (so accidental resets are recoverable with
  a single `mv`). Surfaced on the token-stats man page as a
  "[Reset log…](web2qml://stats/tokens?action=reset_prompt)"
  link at the bottom. The reset_prompt URL renders a
  confirmation page with explanation + recovery command +
  Yes/Cancel links. The `?action=reset_confirm` URL actually
  performs the reset and renders a "done" confirmation page
  with a back link. 3 new unit tests cover: nonexistent log
  returns true (no-op success), normal reset moves to .bak
  with content preserved, second reset overwrites prior .bak.
- [ ] **S — Limit warning toggle.** "Warn me when I cross $X
  in a session." Off by default — user explicitly said they'd
  estimate externally. Easy to add later if requested.
- [ ] **M — Per-workspace breakdown.** Once workspaces ship
  (power-features.md Phase 2), tag events with workspace and
  show "this project has cost $X to summarize" rollups.

### Git first-class integration (v0.1.351 — Phase 1 power-features)

- ✅ **Git auto-detect + status badge** — landed v0.1.351. Branch
  name + dirty/clean + ahead/behind shown in a thin strip above
  the terminal. Hidden in non-repo directories. Async refresh
  via `git_status` module (subprocess `git` calls in a worker
  thread); trigger on cwd-change OR block-close. Tap badge to
  copy branch name. See `docs/power-features.md` "Git first-
  class integration" for the broader arc.
- ✅ **Voice "what branch am I on"** — landed v0.1.354. New phrase
  set (17 variants: "what branch am I on", "current branch",
  "git branch", "show my git status", etc.) routes through a
  `\x01GITBRANCH\x01` sentinel. QML reads `backend.git_badge_text`
  live and surfaces it as a toast ("On main ●" / "Not in a git
  repository"). Works on either tab.
- ✅ **Voice "switch to branch X"** — landed v0.1.355. Patterns:
  "switch to branch X", "checkout X", "check out X" (STT often
  splits), "go to X branch", "switch to X", etc. Gated to
  terminal-tab + in-git-repo (uses git_badge_text liveness).
  Stop-words list rejects "switch to terminal"/"browser" etc.
  so tab-switch utterances aren't hijacked. No pre-validation
  — if the branch doesn't exist, git emits its own error.
- ✅ **Pre-validation for branch switch** — landed v0.1.356. The
  git_status worker now also populates `local_branches`; the
  switch-branch voice handler reads this in-memory snapshot to
  decide whether to surface `git checkout X` (branch exists),
  or `git checkout -b X` as primary + literal checkout as
  secondary (branch missing, likely user wants to create), or
  fall back to v0.1.355 behavior (snapshot empty, let git
  error). No subprocess on the hot path — uses the same worker
  that already runs for the badge.
- ✅ **Smart git command parsing** — landed v0.1.356. 47 phrase
  patterns covering: show diff (incl. --staged), show status,
  show log, show last commit, list branches (incl. -a), undo
  last commit (soft reset preserves changes), stash, pop stash,
  create branch X. Voice → exact git verb. Gated to terminal
  tab + in-repo. Filed mutating-confirmation as a follow-up.
- ✅ **Visual destructive-op signaling in voice picker** — landed
  v0.1.358. Added `destructive: bool` field to `Candidate`
  struct + `Candidate::safe()` / `Candidate::destructive()`
  constructor helpers. Wire format extended with a 7th `\x1F`-
  separated field ("0"/"1"). QML parses with default false for
  back-compat. Picker renders destructive candidates with amber
  border at rest + a `⚠` warning glyph rectangle (with hover
  tooltip "Destructive — mutates state") between the score pill
  and description. Mutating verbs from v0.1.356/357 (undo last
  commit, stash, pop stash, create-branch, amend variants) are
  tagged accordingly. 3 unit tests pin the wire format.

  - ⚠️ **Lesson (logged from v0.1.362 hotfix)**: the bulk-edit
    audit used substring `'reason:'` to check candidate-literal
    coverage. Rust field shorthand syntax (`reason` instead of
    `reason: reason`) defeated the check; 2 sites in production
    code passed the audit but failed to compile. Fixed by hand;
    next bulk audit should also look for `reason,` / `reason\s*}`
    OR (better) parse with `syn` properly.
- [ ] **S — Switch candidate-audit to AST walk.** When the next
  Candidate-schema change happens (probably v0.1.36x's
  "destructive tier" addition for hard-reset-class verbs), use
  `syn::parse_file` + a visitor to enumerate Candidate literals
  rather than regex. One-shot cost; avoids the v0.1.362-class
  miss. Filed as S because the regex path is acceptable for now —
  this is a "if we do another schema migration" task.
- ✅ **More git verbs** — landed v0.1.357. Added: amend (with
  separate "amend with new message" variant), blame, log search
  (`git log --grep`), file history (`git log --follow`). Each
  uses `starts_with` arg-extraction; the new patterns run
  BEFORE the v0.1.356 contains-list so "show the log of foo.rs"
  reaches file-history instead of the generic "show the log".
- ✅ **Even more git verbs (round 1)** — landed v0.1.360. Added
  reflog, tags (list / create / delete), remotes, conflicts,
  fetch, pull, and cherry-pick. Cherry-pick uses hash-shape
  detection (7-40 hex chars) so STT-spelled-out hashes are
  rejected cleanly. Pull + cherry-pick + tag-delete tagged
  destructive (visual signaling from v0.1.358). Fetch + tag-
  create NOT tagged destructive (trivially reversible).
- [ ] **M — Even more git verbs (round 2).** Still missing:
  rebase (`git rebase <ref>` — needs confirmation tier
  beyond just destructive flag, because rebase mid-flight is
  hard to recover from), bisect setup (`git bisect start`,
  step / good / bad), push (never from voice — too high-
  blast-radius), git merge (mutating, can conflict), force-
  push (NEVER from voice; add to a "refuse list" so we
  proactively decline).
- [ ] **M — Git output integration with block tracker.** `git
  status` output → highlight files clickably via existing
  smart-token mechanism. `git log` output → clickable commit
  hashes that drop into `git show <hash>` on tap. `git diff`
  output → syntax-aware coloring via summary path.
- [ ] **L — Branch-aware shell history surfacing.** The
  killer feature from power-features.md. On `git checkout
  <branch>` (detected via cwd change OR command parse),
  scan shell history for entries with the same cwd AND a
  timestamp during the user's prior time on this branch.
  Surface as palette suggestions. Requires shell-history
  parsing infrastructure (atime indexing). Big ship, design
  pass needed.
- [ ] **M — Pre-commit hook awareness.** "Commit this" →
  check `.git/hooks/pre-commit`, run it visibly, surface
  failures before invoking `git commit`. Or refuse politely:
  "pre-commit failed (3 issues); run anyway?"
- [x] ~~**S — Git command output detection.**~~ Done v0.1.885,
  but corrected from the original framing. The literal idea —
  "narrow to git-prefixed blocks" — is wrong: a non-git command
  like `echo x >> tracked.txt`, `rm file`, or `mv` also mutates
  git state, so gating on `git ` would miss real changes. Instead
  the block-close refresh is now gated on `cmd_classifier::classify(
  closed_command).should_snapshot()` — i.e. it fires for Mutating
  and Unknown commands but skips the `git status` subprocess after
  the short, authoritative ReadOnly set (ls/cat/grep/cd/...). Because
  the classifier bumps redirections and pipes-to-file to Mutating,
  `echo x >> tracked` still triggers a refresh. cwd changes still
  refresh unconditionally, and any stale status self-heals on the
  next mutating command or cd. BlockTracker flags `git_relevant_close`
  at block close (where it already classifies for snapshots) and
  main.rs `take()`s it each pty tick. Tested: read-only closes don't
  request a refresh, mutating closes do, and the redirect case is
  covered explicitly. Saves a `git status` fork after the most common
  interactive commands (the prior code forked on EVERY block-close,
  deduped only by an in-flight guard).

### Validation suite gaps

- [x] ~~**S — Golden harness build broke silently (qml_gen output safety
  net down since v0.1.884).**~~ FIXED v0.1.888. `scripts/qml_golden.sh`
  stages a dependency-free copy of qml_gen + its sibling modules and diffs
  the emitted QML byte-for-byte against committed goldens — the intended
  safety net before any qml_gen refactor. But it stages a fixed module
  list, and it stopped compiling once qml_gen gained `crate::util::
  safe_byte_truncate` (v0.1.884) and `crate::clog!` calls that weren't in
  the staged closure — so it had been failing to BUILD (and thus silently
  not running) for several versions. Because it isn't wired into
  `validate.sh`, nothing flagged it. Fix: stage `util.rs` (std-only, clean)
  and add the same `clog!` stub the pure-test harness uses. Reconciled the
  one resulting drift — `edge_cases.html` rendered a `javascript:void(0)`
  href as a clickable link in the stale golden, but current code (v0.1.875
  `is_dead_href`) correctly renders dead/JS hrefs as a de-emphasized label
  + ⚡ marker (unit-tested) — so the golden was stale, not the code; updated
  it. Also added a `bg_image.html` fixture so v0.1.887's background-image
  extraction gains coverage (verified the emitted output: 3 leaf bg-image
  divs → 3 Images; hero-with-text → Text only; gradient/data:/cursor →
  nothing). Harness now green, 10/10.

  - [x] ~~Follow-up: harden so it can't silently rot~~ DONE v0.1.890, both
    halves: (1) **Auto-derive** — the staged module list is no longer
    hand-written; qml_golden.sh walks `crate::<mod>::` references
    transitively from qml_gen and stages the closure (derived: qml_gen +
    ad_filter + anim_detect + layout_detect + table_ops + util). A new
    pure-module ref is picked up automatically; a ref to a module needing
    external deps fails the build loudly instead of rotting. (2) **Wired
    into the gate** — validate.sh runs the harness as its final step, so
    output drift OR a harness build-break now fails the ship gate. Staging
    build cache persisted under /tmp (was wiped every run) → ~60s cold (first
    run of a session), ~5s warm; `WEB2QML_SKIP_GOLDEN=1` skips with a visible
    notice. Caught + fixed a bug the persistence introduced (the awk split
    appends, so got/ must be wiped each run), proved idempotency, and proved
    the gate fails on injected drift and recovers on revert.

- [x] ~~**XS — Verify `table_with_bold_in_cell` test assertion
  (manpages.rs:~2485).**~~ DONE v0.1.871. Confirmed: the test WAS
  failing — it asserted `<b>emphasis</b>` but the to_html table path
  (emit_table_html → render_inline) emits `<strong>` (manpages.rs:2241).
  The prior reading was exactly right. Fixed the assertion to
  `<strong>emphasis</strong>` and corrected the misleading comment; both
  it and the QML-path `table_with_bold_in_cell_emits_rich` (`<b>`) now
  pass. The new `table_with_strikethrough_in_cell` (`<del>`) was already
  correct.

- [ ] **Webpage alignment "far to the right" — diagnostic
  shipped v0.1.399, root cause still unknown.** User
  reported repeatedly: pages render with content far to
  the right, only a sliver visible on the left. Couldn't
  reproduce in-sandbox or pin the cause from screenshots.
  The wrapper chain (browseScroll → browseFlick
  contentWidth=scroll.width → Item → Loader width=parent
  → generated Column x:0 width:parent.width) all looks
  correct, so the offset must be inside the generated QML
  — a child emitting a large/negative x. v0.1.399 adds an
  overflow detector in `pageLoader.onLoaded` that logs
  `[align] content overflow: childrenRect.x=… width=…
  loaderWidth=… item.width=… url=…` via
  `backend.log_message` whenever the loaded content's
  childrenRect exceeds the viewport horizontally (x < -5
  or right edge > loaderWidth + 50). NEXT REPRO: capture
  that log line — it identifies the offending geometry and
  the URL, which is the evidence needed to find the emit
  site in qml_gen.rs. Still also want the temp QML
  (`/tmp/web2qml_pv_*.qml`) if available.

- [x] ~~**S — Char-boundary slicing audit of remaining sites.**~~
  Done v0.1.884. Centralized a `util::safe_byte_truncate(s, n) -> &str`
  helper (snap the cut down to the nearest char boundary; returns the
  whole string when it fits, `""` at n=0) with multibyte tests (é, em-
  dash, 🎉, and the historical `havenât` mojibake — none panic, every
  result is valid UTF-8). Audit findings on the listed sites:
  - `qml_gen.rs` URL byte-caps (the v0.1.396-era `1751`/`1945`, now
    `&r[..r.len().min(50/40)]`) — GENUINELY unsafe (`r` is a resolved
    URL/`&str`; an internationalized or mojibake URL could split at the
    cut). Fixed → `safe_byte_truncate`.
  - `audit_log.rs:980-981` SHA-hex truncation — defensive only (hex is
    ASCII) but switched to the helper anyway; a corrupted log can't
    panic the chain-verify path now.
  - `security.rs:926` and `qml_gen.rs:1452/1476` (`is_tracking_attr`) —
    NOT a risk: these operate on `&[u8]` byte slices, where indexing is
    always valid. The original concern assumed `&str`; left as-is.
  - `audit_log.rs:1337` (`&line[..line.len()-1]`) — test-only and guarded
    by a preceding `ends_with('\n')` assert (the stripped byte is always
    the 1-byte newline). Left as-is.
  Also DRY'd the original snap-back loop in `qml_gen.rs:183` (the
  reference implementation) to call the shared helper. Two extra
  find/depth-scan sites surfaced by a broad grep (`qml_gen.rs:1239`,
  `4500`) were checked and are boundary-safe (indices land just past an
  ASCII brace / come from `find`). No remaining unsafe `&str` byte
  slices in these files.
  v0.1.396 fixed archive.rs:302 and js_analyze.rs:80 (real
  user-hit panics on The Verge article with mojibake
  `havenât`). Remaining sites that do byte-arithmetic
  slicing on `&str` and are theoretically unsafe but
  haven't been observed to panic in practice:
  - `audit_log.rs:980-981` — `&expected_prev[..8.min(...)]`
    slicing first 8 chars of SHA hex; ASCII in practice
    but defensive bug
  - `audit_log.rs:1336` — `&line[..line.len()-1]` strips
    last byte; could split multibyte last char
  - `security.rs:926` — `&span[..p.len()]` prefix check;
    p could end mid-codepoint of span
  - `qml_gen.rs:1751, 1945` — `&r[..r.len().min(N)]`
    byte-cap on extracted text
  Pattern: add `is_char_boundary` snap-back loop, or
  centralize a `safe_byte_truncate(s, n) -> &str` helper.
  Filed as S since the work is repetitive but each site is
  small and well-scoped.

- ✅ **Add `cargo check` to the validation suite** — landed
  v0.1.395. New `scripts/validate.sh` runs brace-check →
  qt_struct_check → qml_lint → `cargo check
  --features="voice claude"` in order (fastest first, all
  failures reported). Exits non-zero on any failure with a
  "DO NOT SHIP" banner. The brace-check tools were designed
  to be fast (avoiding cargo's compile cost for per-ship
  iteration) and still serve that purpose during
  edit-loops; validate.sh is the gate that runs once before
  zip-build. Author note: I (Claude) cannot execute
  validate.sh here since the sandbox lacks cargo, but it's
  ready for the user (or future Claude session with cargo
  available) to run. Catches the gap that bit
  v0.1.386→v0.1.392 (`dirs` missing-crate break) and any
  future type errors / unresolved imports /
  Cargo.toml inconsistencies.

### Niftyplot UX feedback (filed v0.1.397)

User feedback after using v0.1.397: three real issues with
the niftyplot / mermaid diagram experience.

- [ ] **S — sequenceDiagram falls through to literal node
  parsing.** User reported: `sequenceDiagram → participant
  B as ... → B->>A` in a doc doesn't create the reference,
  it adds nodes literally. Root cause: mermaid.rs:204
  `parse_header` only accepts `graph` / `flowchart`. When
  `sequenceDiagram` header appears, parser falls through;
  `try_parse_edge_line` doesn't recognize `->>` / `-->>`
  arrows; whole line goes to `try_parse_node_decl` which
  matches the bare identifier `B`. Result: nodes named "B",
  "A", "participant" appear in the canvas with no edges.

  - ✅ Step 1 (v0.1.398): Parser + textual fallback. New
    `src/sequence_diagram.rs` (~440 lines + 18 tests).
    Recognizes header, `participant X as Label`,
    `participant X`, `actor X`, all four arrow styles
    (`->>`, `-->>`, `->`, `-->`), `Note left of`/`right
    of`/`over` directives, `%%` comments. Auto-creates
    participants from message endpoints (mermaid's
    permissive behavior). Detection wired in BOTH the
    `web2qml://mermaid` and `web2qml://niftyplot` URL
    intercepts in main.rs — when source is a sequence
    diagram, routes to a textual fallback rendered
    through `manpages::to_qml` instead of the cards
    canvas. Stops the broken literal-node behavior.
  - [ ] Step 2 (M): Visual renderer. Participants as
    vertical lifelines (evenly-spaced X), messages as
    horizontal arrows ordered top-to-bottom by document
    order. Different topology from cards canvas; needs
    its own render path (lifelines aren't cards).

- ✅ **Hold-tap / right-click to unpin.** Landed v0.1.398.
  `cardDrag` MouseArea now accepts `Qt.LeftButton |
  Qt.RightButton`. Right-click fires `_unpinThisCard()`
  immediately on press. Long-press (Qt's `pressAndHold`,
  default 800ms) fires the same handler if no drag has
  begun. `_unpinThisCard()` routes to
  `backend.unsave_transform_pin` for mermaid+sourceFile
  mode or `backend.clear_card_pin` for the global path,
  updates the in-memory `_pinned` map via reassignment
  (forces binding re-evaluation), and toasts "Unpinned
  <label>". Card stays at its current position rather
  than triggering a re-layout — preserves the user's
  pointer/finger context.

- ✅ **Layout overlap / "crunch-style spacing"** —
  partial fix landed v0.1.399. User confirmed the mindmap
  concept "starting to take shape" but "a bit crowded"
  with overlaps. Added `LayoutConfig::adapt_to_graph(
  node_count, edge_count)` — scales canvas up + loosens
  center_gravity as node count grows (multiplicative on
  base values, near-identity at ~12 nodes so small graphs
  are unchanged). Applied to all 3 layout call sites
  (mermaid, niftyplot/mindmap, connections). This is the
  "flow mode" tuning half of the split-case intuition.
  The "strict mode" half (Sugiyama-style layered layout
  for hierarchical graphs) is still filed below as future
  work.

- [ ] **M — Strict-mode layered layout (Sugiyama).** The
  other half of the split-case intuition. For graphs with
  explicit direction (mermaid `graph TD`) + low cycle
  count, a layered layout (rank assignment by topological
  depth + per-rank ordering + edge routing) reads far
  better than force-directed. `MermaidGraph.direction`
  exists but is unused by layout. Real algorithm work,
  deserves its own arc. v0.1.399's adapt_to_graph improved
  flow mode; this would add the literal/hierarchical mode.



### Destructive-action-safety arc

User-requested v0.1.397: "overwriting graphs should at
least ask for voice confirmation with a checkbox that has
a border animation + 10second timeout on do not apply
changes (yes, do not save yet, discard)".

The specific surface that triggered the request: niftyplot
diagrams. Every pin-drag and tilt-change in the mermaid /
niftyplot canvas was immediately rewriting the user's .md
file via `backend.save_transform_pin` /
`backend.save_transform_tilt`. No confirmation, no undo.

- ✅ **Step 1: Pending-transforms structural prep** —
  landed v0.1.397. New `_pending_transforms:
  HashMap<(String, usize), BlockTransforms>` field on
  Backend. `save_transform_pin`, `unsave_transform_pin`,
  `save_transform_tilt` now stage to the pending map
  instead of writing immediately. New
  `commit_pending_transforms` and
  `discard_pending_transforms` qt_methods. New
  `has_pending_transforms` (bool) and
  `pending_transforms_count` (i32) qt_properties with
  shared `pending_transforms_changed` signal.
  `load_transform_pins` and `load_transform_tilt` now
  consult the pending map first, so the canvas reflects
  in-progress edits before disk catches up. Each save
  method TEMPORARILY auto-commits after staging
  (`commit_pending_transforms()` called inline) so v0.1.397
  is structurally complete with NO behavior change vs
  v0.1.396 — pin drags still hit disk immediately. New
  `qml/ConfirmDialog.qml` component (~250 lines) ships
  alongside but is not yet wired in. v0.1.398 will wire
  the dialog and remove the auto-commit.

- [ ] **Step 2: Wire ConfirmDialog to defer commits** —
  ✅ landed v0.1.398. `_transformConfirmDialog` instance
  embedded inside the `cardsCanvas` Popup. New
  `cardsCanvas._requestClose()` function checks
  `backend.has_pending_transforms` — if true, opens the
  dialog; if false, just closes. All four close call sites
  (Esc, voice "close graph", toolbar close button, "open
  source" preview action) rerouted through
  `_requestClose()`. Auto-commit calls removed from
  `save_transform_pin`, `unsave_transform_pin`, and
  `save_transform_tilt`. Dialog wired with:
    - onConfirmed → `backend.commit_pending_transforms()`
      then close + success toast
    - onDiscarded → `backend.discard_pending_transforms()`
      then close + info toast
    - onCancelled → leave dialog dismissed, canvas stays
      open with pending state preserved
    - 10s timeout default action = discard (per user spec)

- [ ] **Step 3: Voice control while dialog open** —
  v0.1.399. New qt_property `_confirm_dialog_active`. Voice
  parser, when the property is true, routes:
    - "yes" / "save" / "save it" / "confirm" / "ok" → confirm
    - "no" / "don't save" / "wait" / "cancel" → cancel
    - "discard" / "throw it away" / "reset" → discard
  These phrasings would collide with normal speech if
  always-on, hence the dialog-active gate.

- [ ] **Step 4: Apply the pattern to other destructive
  actions** — S each. Candidates: deleting a workspace,
  overwriting a bookmark with merge, clearing the audit
  log, deleting a saved session summary. Each one wires
  ConfirmDialog with its appropriate label + timeout
  action.

### Drag-and-drop + file open arc

Filed v0.1.394. The browser path can already load
`file://` URLs (any local HTML/MD); what was missing was
the input surface — dropping a file into the window or
voice-asking for a file picker.

- ✅ **Step 1: DropArea + voice "open file" dialog** —
  landed v0.1.394. New top-level `DropArea` in QML accepts
  `text/uri-list` MIME drops; first URL is dispatched
  through new backend `handle_dropped_path` qt_method which
  classifies via pure helper `dispatch_dropped_path`:
  Renderable (html/htm/md/markdown/gmi/txt → file://
  navigation) / Directory (file://path/ for the listing
  fallback) / OpenExternal (xdg-open fire-and-forget) /
  NotFound. New `Intent::OpenFileDialog` voice intent
  (phrasings: "open file", "pick a file", "browse files",
  "file picker", etc.) fires `open_file_dialog_requested`
  qt_signal which QML's `FileDialog` (from
  QtQuick.Dialogs) handles. Both drop and dialog routes
  funnel through the same handle_dropped_path call so
  behavior is unified. Visual drag-active overlay shows
  border + "Drop file to open" text. 13 new unit tests on
  the dispatch helper covering: empty/nonexistent/renderable
  extensions (html/md/markdown/gmi)/external extensions
  (pdf/png/no-ext)/directory/file:// prefix stripping/
  case-insensitive ext/path canonicalization.

- [ ] **S — Multi-file drop handling.** Currently first
  URL only. Future: queue subsequent files as new tabs.

- [ ] **S — Drop visual indicator polish.** v1 shows a
  4px accent border + center text. Could be subtler (corner
  badge) or richer (preview the file's icon, show filename).

- [ ] **S — File-drop on the URL bar.** Drop directly onto
  the URL bar should also work. Currently the URL bar's
  hover state would intercept; needs DropArea coordination.

### Discoverability arc

User feedback v0.1.375: "as someone with experience but not a
true power user in every aspect I was having trouble getting
started with the supposed features added for power users."
Each ship has been good locally but the discovery story for
each new feature has been ad-hoc — voice intents are only in
the parser, URLs only in the URL handler, hand-editable
configs in changelog text. Eight ships of power-features in,
deferred-doc-debt is real.

The fix: every power-feature ship gets a **discoverability
gate** alongside the existing tests-pass / changelog-written
requirements. Features that don't surface to a user who doesn't
read changelogs aren't complete.

The unified surface: **`web2qml://recipes`** — single URL,
auto-generated, linked from places users already are.

- ✅ **v0.1.376 — Recipes page tracer-bullet** (S+).
  `src/recipes.rs` module generates a markdown page covering:
  - Every voice intent (programmatically from
    `voice_intent::VOICE_RECIPES` static catalog)
  - Every `web2qml://` URL (hand-curated table)
  - Every hand-editable config file in
    `~/.config/web2qml/`
  - Special-input syntax (fs_find globs, gemini:// URLs,
    file paths in URL bar)
  - Links out to F1 chord overlay and settings popup

  Linked from THREE places:
  - **First-launch hint pill** (training wheels) — animates in
    2.5s after launch, dismissable, calls
    `backend.recipes_mark_seen()` whether tapped or dismissed
    so it only shows once
  - **Settings popup top** — pinned link "Recipes — what
    web2qml can do" with the same blue palette as the
    pill, so the two reinforce each other
  - **Chord overlay footer** — small pill at bottom
    pointing to broader discoverability

  Backend support: `recipes_should_offer()` and
  `recipes_mark_seen()` qt_methods, mirroring the existing
  onboarding stamp pattern.

  **Discoverability test** in
  `voice_intent::recipe_catalog_tests::every_user_facing_intent_has_a_recipe`
  enforces sync: adding an Intent variant without a matching
  VOICE_RECIPES entry fails the build. The allowlist for
  catch-all variants (Unknown, Freeform, Dictate, LlmFallback)
  is explicit so additions get noticed in code review.

  Plus tests: `no_orphaned_recipes` (catches typos in catalog),
  `every_recipe_has_at_least_one_example`, `every_recipe_has_what_happens_text`,
  6 render-output tests (output non-empty, contains known
  voice intents / URLs / config paths / glob syntax / category
  headers).

- ✅ **Programmatic URL_RECIPES coverage check** — landed
  v0.1.383. Different shape than the original "auto-codegen
  the URL_RECIPES list" pitch: we ENFORCE coverage rather
  than generate the list. New test
  `recipes::tests::every_main_rs_url_has_a_recipe` walks
  `include_str!("main.rs")` for `"web2qml://*"` string
  literals via byte scanner (no regex), strips trailing `/`
  and query fragments, asserts each URL maps to a
  URL_RECIPES entry — UNLESS in an ALLOWLIST for internal
  routes that take query strings and aren't user-typeable
  (currently `web2qml://mermaid` and `web2qml://niftyplot`,
  which are emitted by qml_gen for fence rendering, not
  user destinations). Adding a new web2qml:// route without
  a URL_RECIPES entry now fails the build with an
  actionable error message listing exactly which URLs are
  missing.

- ✅ **Context-aware "what can I do here" voice intent** —
  landed v0.1.385. New `Intent::ContextHelp` with phrasings
  "what can I do here", "what's available", "what now",
  "show shortcuts for this page" etc. Distinct from
  ShowRecipes (which is the full catalog): this is a focused
  4-6 item list for the active surface. Routes through new
  `Backend::show_context_help(tab, reading_open)` qt_method
  that calls `recipes::render_context_help` (URL + tab +
  overlay state → markdown). Surface buckets: reading
  overlay (highest precedence) → sessions page → recipes
  page → audit page → terminal tab → generic browser. Every
  bucket ends with a breadcrumb to the full recipes
  catalog. 7 new unit tests covering each bucket + the
  "always points to full catalog" cross-cutting check.

- ✅ **Programmatic config-file catalog** — landed v0.1.388.
  Different shape than original "pull from manifest" pitch:
  same approach as the v0.1.383 URL_RECIPES coverage check —
  we ENFORCE coverage rather than generate the list. New test
  `recipes::tests::config_files_catalog_covers_known_editable_files`
  maintains a `KNOWN_USER_EDITABLE` allowlist of files users
  should be able to hand-edit (bookmarks.toml, settings.conf,
  speakers.toml, reading_calibration.toml, palette.txt,
  wake_phrase.txt, filler_words.toml, user_tags.toml,
  token_log.jsonl, workspaces/, gemini/) and asserts every
  one has a peer entry in CONFIG_FILES. Adding a new
  user-editable config without a CONFIG_FILES entry now
  fails the build with an actionable error. Internal-state
  files (`.bin`, `.flag`, `.jsonl` non-user files, *.log)
  intentionally not in the allowlist — they're not for hand
  editing. Closes the third deferred discoverability
  follow-up. v0.1.388 added bookmarks.toml + user_tags.toml
  entries to CONFIG_FILES while threading the test in.

- [ ] **Recipes-as-data for voice intent: "what can I do"**
  (S). Voice command that opens recipes page filtered to the
  current context. "What can I do here" → if reading, scroll
  to Reading section; if in terminal, Terminal section; etc.

- ✅ **Voice route to recipes** — landed v0.1.377. New
  `Intent::ShowRecipes` variant. Phrasings: "show recipes",
  "what can you do", "what can web2qml do", "what can I say",
  "help", "show capabilities". Dispatches to
  `web2qml://recipes`. Fourth discovery surface (alongside
  first-launch pill, settings link, chord overlay footer).
  Recipe entry includes itself in the catalog, completing the
  self-referential loop: users learn voice recipes by speaking
  the recipe that opens recipes.



### Session arc

User direction v0.1.378: track terminal/coding sessions as a
first-class concept, and use the session segmentation as the
substrate for command-combo suggestions ("you ran these 4
commands together 3 times — save as a workflow?").

A "session" is a bounded period of work — terminal blocks +
browser tabs + voice utterances that belong together. The
build currently sees individual events; it lacks the
segmentation concept. Once it has it, useful things become
possible: "what did I do last session," "show me the commands
I run together," "save this as a workflow."

The arc has clear data dependencies (segmentation → query UI →
N-gram detection → proactive suggestion). Ships smallest-first.

- ✅ **Session segmentation heuristic** — landed v0.1.379.
  New `audit_log::session_id_for(now_ms, speaker_id)`
  function manages `SESSION_STATE` (a process-global
  Mutex<Option<SessionState>>). Triggers new session when
  any of:
   * Idle gap > 15 minutes since last audit-log write
   * Speaker identity change (Alice's speaker_id → Bob's
     speaker_id; typed-after-voice with same human does
     NOT trigger; voice-after-typed-baseline also doesn't
     trigger since there's no prior speaker to change FROM)
   * Process restart (in-memory state starts empty)
  Session IDs are unix seconds at session-start
  (human-readable in raw log lines, collision-free given
  15-min minimum gap). New `session_id: u64` field on
  Entry, serialized as `session=<id>` in every audit-log
  entry going forward. ParsedEntry gets a matching
  `Option<u64>` — None for entries written by older
  builds; readers treat None as "pseudo-session 0." New
  `pub fn current_session_id()` accessor for the upcoming
  sessions-page ship. 10 unit tests (segmentation logic,
  serialization regression, accessor states).

- ✅ **`web2qml://sessions` page** — landed v0.1.381. New
  `src/sessions.rs` module reads ParsedEntry slices from
  audit_log, groups by session_id, renders as markdown that
  flows through `manpages::to_qml`. Each session shows date
  (YYYY-MM-DD HH:MM derived from the unix-seconds id via a
  manual civil-from-days algorithm — no chrono dep needed),
  duration (human-friendly `<1s` / `5s` / `1m` / `1h 5m`),
  command count, first/last command, and speaker
  attribution (single or mixed). Sessions render newest-
  first; long sessions (>30 commands) show first 10 + last
  5 with an ellipsis marker. Pre-v0.1.379 entries with no
  session_id bucket into a "Pre-session-tracking" pseudo-
  session at the bottom. Capped at 20 sessions / 2000
  scanned entries for memory. Added to URL_RECIPES so the
  discoverability page lists it. 11 unit tests covering
  empty input, single-session, multi-session ordering,
  pre-tracking grouping, speaker attribution, failed-
  command markers, unicode-safe truncation, duration
  formatting, and known-date formatting.

- ✅ **Voice: "what did I do last session" / "summarize my
  session"** — landed v0.1.382 (partial). New
  `Intent::SummarizeSession` variant. Phrasings: "summarize
  my session", "what did I do today", "what have I been
  doing", "session summary", and several variations. Routes
  to `web2qml://sessions` for v1 — opens the sessions page
  showing the most-recent session at top. The LLM-generated
  summary banner (the "1-2 sentence summary" piece of the
  original spec) is filed as a follow-up; basic routing
  lands now so the voice phrasing works.

- ✅ **LLM summary banner on sessions page** — landed
  v0.1.383. Closes session-arc step 3 fully (voice routing
  landed v0.1.382). New `Backend::refresh_session_summary`
  qt_method spawns a one-shot background thread that calls
  `ollama_query` against a prompt built from the most-
  recent session's commands. Result stored in an in-memory
  `SUMMARY_CACHE` keyed on (session_id, last_entry_ts_ms)
  so a new entry joining the session invalidates the cache
  and triggers re-summarization on next view. Render path
  in sessions.rs picks up the cache hit if present;
  otherwise shows a placeholder banner. QML side: a 3.5s
  Timer reloads the page after navigation so the typical-
  case ollama latency is hidden. Cross-thread signal
  (`session_summary_ready`) declared but not yet wired —
  polling-via-timer is sufficient for v1. 8 new tests
  (prompt builder for short/long sessions + worked-example
  presence, cache round-trip, cache miss on entry change,
  cache overwrite, cache miss on unknown sid, render uses
  cached, render shows placeholder, no-banner for pre-
  tracking pseudo-session).

- [x] **M — Command-combo detection.** Landed v0.1.404.
  New `src/combo_detect.rs` (~330 lines + 16 tests). Pure
  analysis over a `&[ParsedEntry]` slice (no I/O, no Qt).
  `detect_combos(entries, cfg) -> Vec<ComboCandidate>` finds
  repeated N-grams (len 2-5) WITHIN sessions (never across a
  session boundary — that'd be two unrelated bursts), then
  ranks by `score_candidate` which encodes all the design
  heuristics: log-frequency base, keystrokes-saved (×0.5),
  shell-syntax bonus (×15 — pipes/redirects/grep/awk are
  what users misremember), multi-day bonus (+25 plus per-day
  increments — a sequence across days is a real workflow not
  one-off iteration), all-trivial penalty (−60 for pure
  ls/cd/git-status muscle-memory sequences), partial-trivial
  damping (−4 each), and a 3+-command sweet-spot nudge.
  Destructive sequences (rm/dd/drop table/force-push/etc.)
  gated behind a higher occurrence bar
  (destructive_min_occurrences=4) so we don't suggest
  saving `rm -rf` on its third run. Wired into the sessions
  page via `render_frequent_sequences` — a "Frequent
  sequences" section listing up to 5 candidates with
  occurrence/day-span info, a destructive warning marker,
  and a `web2qml://save-workflow?cmds=...` handoff link.
  Section is silent when no candidate clears the bar. The
  save-workflow URL intercept decodes the payload and shows
  a confirmation page (full one-tap save into the workflow
  library is the proactive-suggestion follow-up — the
  existing workflow infra records by-doing, not
  create-from-list). 16 combo_detect tests + 3 sessions
  integration tests (section appears/silent, percent-encode
  round-trip).

- [x] **M — Proactive suggestion toast.** Landed v0.1.406.
  Closes the session arc's core loop. New
  `combo_detect::proactive_suggestion(entries, suggest_at)`
  returns the single best candidate at EXACTLY the
  suggestion threshold (3rd occurrence) — a one-shot
  crossing trigger, not a recurring nag — gated to
  non-destructive, positive-score sequences only (trivial
  muscle-memory and destructive sequences never trigger a
  proactive toast; destructive ones can still be saved via
  the sessions page's explicit ⚠️ affordance). Backend
  `_check_workflow_suggestion` runs on every block close
  (hooked where long-command notifications fire — blocks
  push to pending_notifications unconditionally, so even
  short repeated commands trigger), reads a 200-entry tail,
  dedups via an in-memory `_suggested_workflows` HashSet
  (each pattern nudged at most once per session), and emits
  the new `workflow_suggestion_ready(name, cmds)` signal.
  QML side: a dedicated NON-MODAL `workflowSuggestionToast`
  (distinct from the fire-and-forget `toast`) with Save /
  ✕ Dismiss buttons, 8s dwell, positioned above the plain
  toast slot. Save calls `workflow_create_from_commands`
  and confirms via the plain toast (handles ok/exists/err).
  5 proactive_suggestion unit tests (fires-at-threshold,
  silent-below, silent-above the one-shot crossing,
  never-destructive, never-trivial).

- [ ] **L — Session-aware task tracking.** Tie sessions to
  task labels ("working on the lazy-load placeholder ship"),
  letting users resume across days. Optional LLM-generated
  task summaries at session boundaries. Big design surface;
  defer until earlier ships are real and we have actual
  session data to look at.

- [ ] **L — Cross-session pattern queries.** "How do I
  usually start a debugging session?" — retrieve the
  historical answer from past audit-log slices. Different
  shape than N-gram detection; needs a separate retrieval
  design.



### Bookmarks arc

Filed v0.1.386 alongside the first ship. A small but
infrastructure-laying arc — several future features want a
saved-URL surface to anchor against.

- ✅ **Step 1: Save voice intent + bookmarks file + page**
  — landed v0.1.386. New `src/bookmarks.rs` (~360 lines, 13
  tests). `Bookmark` struct with url/title/saved_at_ms/tags.
  `save_bookmark` appends a TOML-ish `[[bookmark]]` block
  to `~/.config/web2qml/bookmarks.toml` (no dedup at write
  time — intentional, user can re-save with different
  tags). `read_bookmarks` parses, returns newest-first.
  `render_bookmarks` produces markdown that flows through
  `manpages::to_qml`. Hand-rolled quote_toml/unquote_toml +
  parse_tags_array (no serde_toml dep, same approach as
  reading_calibration.toml). civil-from-days date math
  matches sessions.rs format_unix_seconds. New
  `Intent::SaveUrl(String)` (empty string = current page)
  + `Intent::ShowBookmarks` variants with parsers covering
  "save this", "bookmark this", "remember this page", "save
  url <X>", "show bookmarks". Backend `save_url(url, tags)`
  qt_method + `bookmark_saved` qt_signal. New
  `derive_title_from_url` helper (strip scheme, hostname +
  first path segment up to 40 chars, unicode-safe). QML
  dispatch cases with toast on save. New
  `web2qml://bookmarks` URL handler + URL_RECIPES entry
  (passes the coverage check).

- ✅ **Step 2: Tag-based filtering** — landed v0.1.387.
  `web2qml://bookmarks?tag=<X>` URL handler parses the
  query string and routes to
  `bookmarks::render_bookmarks_filtered(tag)` which
  case-insensitive matches each entry's tags. New
  `Intent::ShowBookmarksTagged(String)` voice intent with
  phrasings: "show <tag> bookmarks", "show bookmarks
  tagged <X>", "filter bookmarks by <X>", "filter
  bookmarks by tag <X>", "bookmarks tagged <X>", and the
  bare "<tag> bookmarks" form (single-word, with stop-word
  protection so "my bookmarks" / "saved bookmarks" still
  route to the unfiltered intent). Filter mismatch shows
  a helpful message pointing to the full list. URL coverage
  unchanged since the bare `web2qml://bookmarks` already
  has a URL_RECIPES entry. 5 new tests covering basic
  filtering, case-insensitive matching, no-matches message,
  empty-tag = no filter, and None passthrough.

- ✅ **Step 3: Dedup by URL with merge** — landed v0.1.388.
  `save_bookmark` now reads existing entries, checks for URL
  match, and either merges (union of tags case-insensitive,
  bump saved_at_ms, preserve user-edited title) or appends.
  Returns new `SaveResult` enum (`Fresh` vs
  `Merged { new_tags_added: usize }`) so callers can show
  distinct toasts: "Bookmarked" / "Updated existing
  bookmark" / "Already bookmarked (timestamp bumped)".
  Backend exposes `last_bookmark_save_result` qt_property so
  QML reads which case fired. Full file rewrite on every
  save (cheap for typical-size bookmark files). 8 new tests
  covering fresh vs merge, tag union, no-new-tags case,
  timestamp bump, user-title preservation, empty-title fill,
  case-insensitive tag merge, distinct-URL separation.

- ✅ **Step 4: Title extraction at fetch time** — landed
  v0.1.387. New `qml_gen::extract_title(doc)` pulls the
  `<title>` text from a parsed HTML document, collapses
  whitespace, trims, rejects known placeholders
  ("untitled document", "new tab", "loading...", etc.),
  caps at 200 chars with unicode-safe truncation. Backend
  stores result in new `current_title` String field at
  fetch time. `save_url` now prefers `current_title` over
  the derive_title_from_url fallback when saving the
  current page; parametric saves still use the fallback
  since we don't have a title for URLs we haven't
  fetched. Future ships can wire current_title into
  audit-log display strings, session-summary prompts,
  and tab labels. 9 new tests (basic, whitespace, no
  title, whitespace-only, placeholders, 200-char cap,
  unicode-safe truncation, punctuation preservation,
  nested tags inside title).

- [ ] **M — Sync with archive entries.** Archive contains
  page hashes + extracted key-points; bookmarks contains
  user-flagged URLs. Show "archived snapshot available"
  badge on bookmark entries when the URL is in archive.db;
  let user navigate to the archive view directly. Closes
  a small but real "where IS the archived copy of this
  bookmark" UX gap.

- [ ] **M — Voice "show recent bookmarks" / "what did I
  bookmark today".** Time-scoped queries. Reuses the
  audit-log style time-range parser.



### Workspaces & cross-session state

Filed v0.1.389. The workspaces infrastructure (per-speaker
persistence of browser_tabs + terminal_sessions) has existed
since v0.1.293-294 but lacked an exposure layer — users had
no way to SEE what was saved across speakers or jump between
saved tab sets without speaker-switching.

- ✅ **Step 1: web2qml://workspaces cross-speaker view** —
  landed v0.1.389. New `src/workspaces_page.rs` (~330 lines,
  11 tests). Reads every speaker dir under
  `~/.config/web2qml/workspaces/`, loads each speaker's
  named workspaces, renders sorted-by-updated-ms-descending.
  Each row shows speaker display name, workspace name,
  "last touched X ago" (just-now / Nm / Nh / Nd / Nw / Nmo
  / Ny formatting), tab count, terminal-session count, the
  full tab URL list with ← active marker on the active one,
  and per-terminal cwd + last command. New
  `workspaces::list_speakers()` walks the workspaces root
  for all speaker ids. Added to URL_RECIPES.

- ✅ **Step 2: Voice tab substring-match jump** — landed
  v0.1.389. New `Intent::SwitchToTabMatching(String)` voice
  intent. Phrasings: "switch to the X tab", "go to the X
  tab", "switch to tab with X", "go to tab with X",
  "switch to X tab", "go to X tab". Six prefix patterns
  with disambiguation rules to avoid collision with
  directional tab intents (TabNext/TabPrev/TabClose run
  first via matches!) and with the generic Navigate intent
  ("go to apple.com" must still navigate, not search for a
  tab named apple.com). Defensive rejection of literal
  directional words ("next", "previous", "current", "the
  other", etc.) as queries. Backend
  `switch_to_tab_matching(query)` qt_method scans
  `self._tabs` case-insensitively, first-match-wins,
  returns bool. QML dispatches with success/no-match
  toasts. 8 new parser tests covering each phrasing form
  plus all the disambiguation cases.

- ✅ **Step 3: Voice "save workspace as <name>"** — landed
  v0.1.390. New `Intent::SaveWorkspaceAs(String)` voice
  intent. Phrasings: "save workspace as <X>", "save this
  workspace as <X>", "save my workspace as <X>", "save
  current workspace as <X>", "save the workspace as <X>".
  Dispatches to backend `workspace_save_as` qt_method
  (existed since v0.1.298). Whisper-truncation defense:
  new `is_reserved_workspace_name()` helper rejects "this"
  / "that" / "current" / "the" / "my" / "active" /
  "default workspace" as names — these leak in when
  whisper drops the actual workspace name. Success/failure
  toasts. 4 parser tests.

- ✅ **Step 4: Voice "load workspace <name>"** — landed
  v0.1.390. New `Intent::LoadWorkspace(String)` voice
  intent. Phrasings: "load workspace <X>", "open workspace
  <X>", "switch to workspace <X>", "load the <X>
  workspace", "switch to the <X> workspace", "switch to
  <X> workspace". Dispatches to backend
  `workspace_load_by_name` (existed since v0.1.298) which
  auto-saves the active speaker's current state to default
  before swapping, so unsaved tabs aren't lost. Same
  reserved-name defense as step 3. 6 parser tests
  covering each phrasing form + disambiguation against
  SwitchToTabMatching, SaveUrl, Navigate.

- ✅ **Step 5: Tab-open timestamps** — landed v0.1.391.
  Different shape than originally filed: rather than
  growing `_tabs` from `Vec<String>` to `Vec<TabState>`
  (44 reference sites would have needed touching), used
  parallel-arrays pattern with new `_tab_opened_ms:
  Vec<u64>` mirroring `_tabs` at every mutation site (6
  sites: tab_open push, tab_close remove, 3 workspace
  clone sites with now-rehydration, 2 clear sites). New
  `tab_ages_list: QString` qt_property (NOTIFY
  tabs_changed) carries newline-joined short-form ages
  parallel to tabs_list. New `current_ms()` free helper
  (extracts a 6-times-duplicated inline pattern). New
  `format_age_label(opened_ms, now_ms)` produces short
  strings: "now" / "Nm" / "Nh" / "Nd" / "Nw" / "Nmo" /
  "Ny" / "?" (zero or future). `_publish_tabs` publishes
  ages on every tab change. Not persisted to disk yet —
  workspaces TOML schema would need growing; deferred to
  a future ship. After process restart, tabs get
  load-time as their opened_ms. 8 new unit tests
  covering all ranges + defensive zero/future inputs.

- ✅ **Step 5b: Workspaces page injects current session** —
  landed v0.1.392. The v0.1.389 workspaces page showed only
  on-disk persisted workspaces; a user with several tabs
  open RIGHT NOW didn't see them on the page until after
  saving. Fixed by adding a "Current session" section at
  the top of the page with the live tabs + per-tab ages
  (from v0.1.391 `_tab_opened_ms`). Backend builds a new
  `workspaces_page::LiveSession` struct from `_tabs`,
  `_tab_opened_ms`, and `speakers::current_active_id` at
  the URL handler; passes to new
  `render_workspaces_page_with_live`. Single-tab sessions
  hidden (matches strip's hide-when-single behavior).
  Section omitted entirely when no active session has ≥2
  tabs. Refactored `format_ago_ms` to delegate to
  `format_relative_ago(ts, now)` so a single render pass
  uses one consistent "now" across rows. 6 new tests
  covering the live section: None / single-tab filtered /
  two-tabs shown / active marker / empty-speaker no
  dangling-dash / live-before-saved ordering + 1 test
  confirming format_relative_ago uses the passed-in now.

- ✅ **Step 5c: QML tab-age tooltip** — landed v0.1.392.
  Surfaces the v0.1.391 `tab_ages_list` qt_property in the
  tab strip. New `_ages` parallel JS array on the tabStrip
  Rectangle. New ToolTip on each chip showing full URL +
  per-tab age ("opened just now" / "opened 5m ago" /
  "opened at unknown time"). 600ms delay (Qt default),
  4s timeout. Works on touch via long-press. Distinguishes
  chips when many tabs collide on the same short label
  ("github.com" appearing on 4 chips).

- [ ] **Step 6: Cross-speaker tab dedup awareness** (M).
  When loading another speaker's workspace, the active
  speaker may already have one of those tabs open.
  Currently they get a duplicate. Add a "switch to
  existing instead of dup" hint in the workspace load
  flow.



### JS-rendering arc

How web2qml handles single-page applications and JS-rendered
content. Filed during the v0.1.366-v0.1.367 design conversation
that also produced the federation arc. Three escalating
approaches, each with very different scope.

- ✅ **Option 1 — SPA detection banner** (S). Landed v0.1.373.
  `qml_gen::detect_spa(html)` recognizes pages with empty
  `<div id="root">`/`<div id="app">`/`<div id="__next">` mounts
  + scripts + minimal body content. Returns a friendly message,
  framework-specific when detectable (Next.js, Nuxt, React, Vue,
  Svelte, Angular). main.rs's fetch path renders a banner with
  the message + "Open in browser" button instead of an empty
  page. 6 unit tests covering positive and negative cases. Comes
  AFTER challenge detection so Cloudflare-protected SPAs still
  show challenge UI.

- 🟡 **Option 2 — Static hydration extraction** (M). Partial
  landing v0.1.375. `qml_gen::extract_hydration_data(html)`
  recognizes and extracts JSON blobs from:
  - `<script id="__NEXT_DATA__" type="application/json">`
    (Next.js) — most common
  - `window.__NUXT__ = {...}` assignment (Nuxt 2)
  - `window.__INITIAL_STATE__ = {...}` (generic Redux/Vuex)
  - `<script type="application/ld+json">` (last resort
    fallback for structured data on any page)

  Returns `(kind, json_string)` where `kind` is a short
  identifier ("next", "nuxt", "initial-state", "json-ld") so
  future rendering paths can switch on it. 9 unit tests cover
  positive cases, attribute-order tolerance, priority ordering
  (Next.js wins over JSON-LD when both present), brace-aware
  parsing (strings containing `}` don't fool the depth counter),
  and negative cases.

  **PARTIAL RENDERING (v0.1.378)**: `json-ld` kind now renders.
  `qml_gen::render_json_ld_article(blob)` extracts schema.org
  Article / BlogPosting / NewsArticle / TechArticle /
  ScholarlyArticle / Report shapes — pulls headline, author
  (handles nested `{"@type":"Person","name":"..."}` form),
  datePublished, description, articleBody. Lightweight
  byte-walker (no serde_json dep) with escape-aware string
  extraction and \uXXXX unicode decoding. Returns markdown
  that flows through `manpages::to_qml` — same rendering
  pipeline as built-in manpages, gemini archive, and
  recipes. Wired into main.rs fetch path BEFORE the SPA
  banner: when JSON-LD parses successfully, that's the
  rendered page; when it doesn't, fall through to banner.
  12 new unit tests in `qml_gen::tests`.

  **NOT YET DONE in Option 2**: ✅ landed v0.1.385.
  `qml_gen::render_nuxt_article` and
  `qml_gen::render_initial_state_article` both delegate to
  the v0.1.384 `render_next_data_article` shape-walker since
  the detection is field-pattern-based, not path-based.
  Container varies massively per Nuxt version + per-app store
  layout, so walking for the title+body pair regardless of
  path covers Nuxt 2 (`__NUXT__.data`), Nuxt 3
  (`__NUXT__.state.*`), Pinia stores, and Redux/Vuex
  `__INITIAL_STATE__` shapes uniformly. 5 new tests
  (Nuxt 3 state container, Pinia store shape, Redux store,
  no-article-shape negative case, empty state negative).

- [ ] **Option 3 — Embed JS engine** (L+, 2-3 months). Vendor
  `boa-engine` (~2MB Rust JS interpreter) with a synthetic
  DOM bridge. The "becomes-a-browser" concern: maintaining a
  JS+DOM environment that real-world sites work against is
  the work of a browser engine team. Recommendation per the
  design conversation: ship Options 1 and 2 first, gather
  data on what users actually hit, only then decide whether
  Option 3 is justified.



### Small-web protocols + federation

The thread that started with "let me find old hosts via 70s
protocols" and grew into a federated knowledge graph design.
Multi-quarter arc; see `docs/federation-design.md` for the full
specification.

- ✅ **v0.1.367 — Gemini archive viewer** (tracer bullet).
  `src/gemini.rs` module: Gemtext parser, archive layout,
  launcher renderer. `web2qml://gemini` shows launcher with
  archived capsules + curated starting points + how to populate.
  `gemini://*` URLs resolve from local archive. User pre-loads
  archive with `gemget` / `amfora`. 24 unit tests. No network
  code yet — pure rendering pipeline proof.

- [ ] **v0.1.368 — Gemini live fetch + TOFU pinning** (M). Add
  `rustls` with custom `ServerCertVerifier`. TLS to port 1965,
  request, response parse. Per-host fingerprint pinning with
  history tracking. Auto-save fetches to archive. New
  `web2qml://gemini/certs` page for cert history.

- [ ] **v0.1.369 — Public-flag corpus + Gemtext export** (M).
  Add `public: bool` to corpus items. Voice "make this public".
  Background exporter to
  `~/.config/web2qml/published/<id>.gmi`. No serving yet —
  files on disk.

- [ ] **v0.1.37x — LAN-only Gemini server + mDNS** (M). Small
  Gemini server serving from `~/.config/web2qml/published/`.
  Self-signed cert, persisted. mDNS announce
  `_gemini._tcp.local` for LAN discovery.

- [ ] **v0.1.37x — Manual curator add + data model** (M).
  `~/.config/web2qml/curators.toml`. `web2qml://curators` UI
  for paste-based add. Periodic background fetch of curator
  feeds. Voice queries surface curator publications.

- [ ] **v0.1.37x — Link graph foundation** (M-L). `[[wikilink]]`
  convention. Forward/back-link index. Centrality. Voice "what
  links to this?"
  - `[[wikilink]]` + forward/back-link index: DONE earlier (`link_graph.rs`
    `LinkGraph`, `backlinks.rs`). "what links to this?" data is `backlinks()`.
  - Centrality + walkthrough: DONE v0.1.894. `degree_centrality` /
    `centrality_ranking` (hubs first, stable ties) and `walkthrough()` — a
    deterministic guided tour that starts at the densest hub, walks its
    neighbourhood breadth-first in centrality order, then jumps to the next
    cluster's hub (disconnected components covered, no interleaving). Exposed
    LIVE via the `manpage_graph_walkthrough` qt_method
    (`{"order":[...],"hubs":[...]}`) over the help corpus — first real consumer
    of the walkthrough. 8 unit tests (hub ranking, full coverage, component
    ordering, json/empty).
  - The walkthrough is **parser-agnostic**: anything that feeds (node, links)
    into a `LinkGraph` gets a tour for free. Two follow-ups this unlocks, both
    requested:
    - **Graph-walkthrough × email**: DONE v0.1.895. `LinkGraph::from_niftyplot`
      turns any niftyplot graph source into a link graph (node ids → slugs,
      directed edges → forward links), reusing `add_document`'s index logic; it
      normalizes endpoint ids to absorb the parser's `: "label"` edge-suffix
      quirk. Exposed LIVE via the `niftyplot_walkthrough` qt_method, which
      returns `{"order":[{slug,label}...],"hubs":[{slug,label,degree}...]}`.
      Feed it `mail_panes::sender_graph_niftyplot` / `attachment_graph_niftyplot`
      and it tours the inbox from "You" outward to each correspondent. Pure over
      its input string → runs on note/terminal niftyplot blocks today, and on the
      live inbox graph the moment that (auth-gated) source is produced. Unit test
      drives the real inbox-sender DSL end-to-end. Still TODO: a QML affordance
      that calls it from the niftyplot canvas / a "walk me through this graph"
      voice phrase.
    - **Multi-user**: when cards/notes carry a speaker id (see "Speaker id in
      cards", 918), centrality can be computed per-author or the walkthrough
      seeded from a chosen author's hub, so a shared device can tour "Dad's
      notes" vs "Mara's notes" over the same graph. Attribution arc is the
      gate; the graph side is ready.

- [ ] **v0.1.37x — Link metadata in exports** (S). Published
  Gemtext gains a metadata footer with strength scores. Private
  link counts surface as anonymous totals.

- [ ] **v0.1.37x — Bluetooth pairing infrastructure** (M-L).
  BLE via `btleplug`. ECDH over GATT. RSSI gate.

- [ ] **v0.1.37x — Acoustic OOB integrity** (M). Hash of
  `(pks, voicefps)` encoded as ~4s of audio. Each device emits;
  other verifies. Defeats Bluetooth MITM.

- [ ] **v0.1.37x — Acoustic challenge-response + emission
  jitter** (M). Back-and-forth instead of broadcast. Emission
  timings derived from `hash(secret, nonce, seqno)` —
  unpredictable to outsiders. Defeats phased acoustic arrays.

- [ ] **v0.1.37x — Room impulse-response fingerprint** (M-L).
  Pre-handshake calibration captures room IR at each device.
  Modal frequencies + T60 exchanged via BLE; both sides verify
  agreement. ToF click pings establish inter-device geometry.
  Handshake hash includes room features. Defeats directional
  acoustic injection from outside the room.

- [ ] **v0.1.37x — Voice phrase callback** (S). Derive a 3-4
  word phrase from handshake state via fixed dictionary. Both
  parties read aloud. ASR + PC1 verify correct words from
  correct speakers. Phrase is public information so reading
  aloud is safe. Friendly social-feeling ceremony, recognizable
  in shared spaces.

- [ ] **v0.1.37x — Integrated handshake UX** (S). Glue.

- [ ] **L+ — Cross-NAT reachability** (Tailscale integration
  or hosted relay).

- [ ] **L+ — Fingerprint federation** (opt-in cert observation
  sharing; community CT for Gemini).

- [ ] **L+ — Federated semantic search** ("voice query across
  all curators").

- [ ] **L+ — Web-of-trust visualization**.

- [ ] **Gopher / Finger / Tilde clients** — minor protocols
  worth adding once Gemini live fetch lands. Each ~S.

### Layout polish — sidebar detection + content-overflow

Long-running thread of small fixes for the "content gets pushed
off-screen to the right" symptom when web2qml renders modern
React/Tailwind sites with sidebar shells.

- ✅ **v0.1.345**: original sidebar-aware demotion. Direct
  children with `<aside>` / `<nav>` / canonical sidebar class
  names trigger Row → Column. Plus 800px x-clamp as backstop.
- ✅ **v0.1.366**: expanded sidebar detection for
  Tailwind/React layouts (chatgpt.com, claude.ai, linear.app,
  notion.so style shells). New signals:
  - Tailwind width utilities `w-44`/`w-48`/.../`w-96` plus
    responsive variants (`md:w-64`, `lg:w-72`)
  - Tailwind position classes `fixed` / `sticky`
  - Inline `style="width: <NNN>px"` ≤ 320px, or `<N>rem` ≤ 20rem
  - Inline `style="position: fixed|sticky"`
  - **Deep-scan fallback**: when direct children don't match
    but the layout is Row-shaped (2-3 element children), look
    ONE level into the first child IF it's a structurally
    neutral wrapper (`<div>` / `<span>` / `<section>` —
    explicitly NOT `<header>` / `<footer>` / `<main>` because
    those imply role, e.g. breadcrumbs).
  - is_sidebar_like extracted as a reusable single-element
    check so future heuristics share the same signal set.
  - 11 new unit tests covering Tailwind widths (positive +
    negative), inline widths (px + rem + skip-wide),
    inline position-fixed, deep scan happy path, deep scan
    refuses semantic wrappers, deep scan child-count cap.

Filed follow-ups:

- ✅ **CSS grid-cols sidebar detection** — landed v0.1.370.
  Sidebar demotion in qml_gen now triggers for
  `LayoutKind::Grid` in addition to Row/RowWrap. The
  has_sidebar_child check is unchanged — it already detects
  Tailwind width classes, fixed/sticky positioning, and inline
  narrow widths that grid layouts use. One new unit test
  (`has_sidebar_child_works_on_grid_layout`) exercises the
  `grid grid-cols-[18rem,1fr]` pattern with a `w-72` first
  child.
- ✅ **Inline-style flex-shrink:0 with explicit width** — verified
  v0.1.377. Behavior was already shipping since v0.1.366's inline
  width-detection pass at line ~961-989 of qml_gen.rs. This ship
  adds 5 dedicated tests covering the common React pattern
  (`style="flex-shrink: 0; width: 280px"`) plus rem widths,
  wide-width rejection, position:fixed, and position:sticky.
- [ ] **M — Stricter overflow handling at the Row layer.**
  Even with correct sidebar detection, some sites produce Row
  layouts with content children wider than the viewport. Could
  emit a `Clip` rectangle around Row containers as a paint-
  level backstop. Risk: paints get cropped instead of
  reflowed; less informative for the user.

### SSH-aware workspace + tool borrowing

User raised this question: "I'm SSH'd into a system and I have
my desktop machine... can I merge my tool pipe calls with a
special operation to bring my system tools over here or is
that a power user thing that they should have set up already?"

Approach: scaled-down "borrow my tools" model — web2qml DOESN'T
build a transparent tool-projection layer (too big, competes
with active arcs). Instead: detect SSH, offer to scp the user's
configured toolkit to the remote on demand.

- ✅ **SSH session detection (foundation)** — landed v0.1.363.
  `BlockTracker::in_ssh_session` set when the most-recent
  submit_command is an SSH-style opener: `ssh`, `mosh`,
  `kubectl exec`, `docker exec`, `docker run -it` (also
  `podman` variants). Heuristic only — covers the common
  cases. Backend exposes `in_ssh_session: bool` qt_property
  with `in_ssh_session_changed` signal; pty_read_screen polls
  block_tracker and emits on transitions.

  UI side-effects when active:
  - git_status_badge hidden (our /proc cwd is misleading)
  - New `sshBadge` Rectangle shows "ssh session — local
    features paused" with a 🔗 glyph
  - long-cmd toast prefixes with "(remote, …)"
  - fs_find files-mode refuses with "local only" status text

  11 unit tests pin the is_ssh_opener classification.

- [ ] **M — Borrow my tools.** Voice command "borrow my tools"
  triggers scp of a configurable binary list (default: rg, fd,
  jq, bat, delta) to the remote's `~/.local/bin/` plus types
  `export PATH=~/.local/bin:$PATH` into the active remote
  shell. Settings panel option to edit the binary list. Voice
  command "return my tools" / "unborrow tools" removes them.

  Implementation needs:
  - Settings field for binary list (TOML list, persisted)
  - SSH connection info reuse — get user@host from the active
    ssh subprocess (parse from the command we already
    recorded in the BlockTracker for the session-opening
    block)
  - scp invocation in a worker thread; progress reporting
  - PATH export typed into the remote shell via pty_write
  - Cleanup path that scp -r removes the binaries

- [ ] **S — Missing-binary → borrow prompt.** When the user
  runs a command on a remote shell that fails with "command
  not found" AND the binary is in the borrow-list, surface a
  toast offering to borrow. Hooks the existing
  missing_packages infra; just adds an alternative action.

- [ ] **L — Transparent tool projection.** The big version:
  `something | rg foo` on remote, web2qml intercepts via
  remote-side helper + reverse channel, runs rg locally on
  streamed data. Filed for completeness; substantial work,
  may never ship if the smaller items prove sufficient.

### Email integration — DEFERRED, hooks-only approach

User raised the question: "would gut email workflow integration
be nice here or is that a let poweruser do that elsewhere?"

**Decision: defer full email client. Build voice/smart-token
HOOKS for existing terminal email clients instead.**

Why defer a full client:
- Power users already have great tools (mutt, neomutt, mu4e,
  aerc, himalaya, notmuch). All mature, all CLI-friendly.
- IMAP/JMAP + OAuth + HTML email rendering + attachment
  handling + SMTP + spam + signing/encryption is MASSIVE
  surface area. Even a "voice-only" subset would dwarf the
  git work we've shipped.
- A naive IMAP client in web2qml would be vastly inferior to
  mature TUI clients. Bad first impression in a saturated
  domain.

What the right approach looks like — same model as git
(web2qml doesn't reimplement git, it makes voice + AI work
great ON TOP of the user's existing CLI):

- [ ] **M — Voice intents for terminal email clients.** Detect
  himalaya/mutt/neomutt/aerc/notmuch invocation (cmd-classifier
  hook). Voice phrases:
  - "what's in my inbox" → `himalaya envelope list` (or
    whichever client is active; persist per-user choice in
    settings.conf as `email_client=himalaya`)
  - "search email for X" → `notmuch search X` /
    `himalaya envelope list --query subject:X`
  - "reply to message N" → client-specific reply command
  - "compose to X" → client-specific compose with To: filled
- ✅ **Email-shaped block detection** — landed v0.1.375. New
  `term_blocks::BlockKind` enum (Plain/Email/Git/Table) and
  `CommandBlock::detected_kind()` method. Email detector requires
  at least one anchor header (`From:` or `Subject:`) plus one
  more supporting header (`To:`, `Cc:`, `Date:`, etc.) within
  the first 10 lines to fire — avoids false positives on prose
  containing a single header-like phrase. Git detector requires
  command-prefix match AND non-empty output (so silent commands
  like `git config` don't tag). Table detector requires 3+
  consecutive lines with consistent column counts (±1 tolerance,
  ruler lines like `---` skipped). 12 unit tests. Future ships
  (smart tokens, AI summary path) read `detected_kind()` to
  branch.
- [ ] **S — Smart tokens for email addresses.** Pattern
  `[a-z0-9._-]+@[a-z0-9.-]+\.[a-z]{2,}` already gets picked
  up by URL detection in some paths. Make it click-to-copy
  in email-shaped blocks; right-click for "compose to" if
  an email client is configured.
- ✅ **AI summary path that knows email shape** — landed
  v0.1.378. New `is_email: bool` field on `SummaryRequest`
  set at `request_summary` call time via
  `term_blocks::CommandBlock::looks_like_email` (the detector
  from v0.1.375). Worker branches on `is_email` before
  `is_diff` to select the prompt template. New
  `build_email_prompt` reframes the question from "what's
  this output telling me" to "what's this email about + what
  does it ask me to do" — and includes a phishing-detection
  example so small models flag suspicious sender domains.
  Cap raised to 6000 chars (vs 4000 for the generic prompt)
  since header + body together benefit from being seen as
  one unit. 6 unit tests in `email_prompt_tests`.

### Terminal interaction polish (continued)

- [ ] **L — Smooth-scroll passthrough for spammed pager keys.** User
  asks: when hjkl/jk/arrow keys are spammed in helix/less, batch
  the navigation and render the transition with a tween between
  old/new screen snapshots. Frame interpolation via minterpolate,
  cross-fade between buffers, or animated text translate. Detect
  rapid repeated keys (>5/sec for same key), batch them, ask the
  pager to scroll the SUM, then animate from pre-image to
  post-image over ~120ms. Distinct from the smooth viewport
  scrolling — that's our own scrollback, this is layered ON TOP
  of the pager's internal navigation.

- [ ] **M — 5-point 5×5 calibration anchors.** User feedback:
  current font calibration measures a single glyph at one position.
  Some fonts/rendering paths produce slightly different metrics
  at different viewport positions (sub-pixel drift, hinting
  variance). Sample 5 anchor points (corners + center) of a 5×5
  test glyph grid; if metrics diverge >0.5px, log and use the
  median. Won't fix anything currently broken; defensive against
  future rendering issues.

- [ ] **M — CSS `display:none` and visibility:hidden propagation
  in browser tab.** User report: "dynamic hiding with css still
  broken". The minimal browser doesn't fully evaluate CSS display
  rules — elements marked hidden via JS toggling style still render.
  Needs investigation of which paths skip the rule check. Repro
  case needed; user to provide page URL.

### Editor integration

- [ ] **L — Helix LSP bridge.** See `../HELIX_integration_design.md`.
  Connect to running helix via socket; read cursor position, current
  file, diagnostics. Voice commands like "fix the warning on line 47"
  become natural.

### Codemap

- [ ] **L — Project structure indexing.** Per-project symbol/file
  cache locally. "Open the file where authentication is handled"
  resolves without full fuzzy search. Needs user spec on what
  granularity (per-symbol via tree-sitter? per-file via tag/comment
  extraction?).

### Shell-history-driven candidate ranking

- [ ] **M — Score candidates by recent shell usage.** User feedback
  (v0.1.123): "show files" has trouble adjusting when the user
  switches from `ls` to `dust` because the decision cache locks
  in the first mapping. Improvement: scan recent shell history
  (already in `cmd_collections["History"]`); boost candidates
  whose command appears in recent history; demote candidates
  whose command hasn't appeared. Compounds with the explicit-
  binary surfacing landed in v0.1.123.

### Browser ↔ terminal context bridge

- [ ] **M — Cross-tab actions.** "Look up that URL with `man`" from
  browser tab. "Search the error on Stack Overflow" from terminal tab.
  Plumbing: shared context object holding most-recent URL, most-recent
  command, most-recent error text.

### Non-HTML document sources (markdown, mermaid, math)

**User feedback v0.1.169 → v0.1.170:** "I was able to view
markdown files, but tables have no rendered structure and math
still which makes them somewhat less useful but at least they
are viewable without friction for now."

So: text-mode markdown reading works. Tables collapse to no
structure (probably rendering pipe-syntax as plain text). Math
falls through as raw `$...$` or `2^r^` markup. User declared
the limited markdown viewing acceptable for now — bumping the
table-rendering and math-rendering work up in priority is
filed but not the current arc.

Immediate filed follow-ups (still L since they touch the same
multi-month tree):
  - [ ] **M — Markdown table rendering with our existing table
    primitive.** Whatever pipeline currently surfaces .md is
    treating tables as text. Walk through and produce
    `Table { headers, rows }` from the markdown pipe syntax,
    then route into the same renderer that handles HTML
    tables. This is a small piece extractable from the larger
    DocNode tree.
  - [x] ~~**M — Math passthrough as styled monospace.**~~ Done v0.1.891.
    `$…$` and `$$…$$` now render as a distinguishable monospace span across
    all three inline paths (the strikethrough precedent — keep them in sync):
    `inline_to_richtext` (QML) emits a mono `<span>`, `render_inline` (HTML)
    emits `<code>`, both KEEPING the `$` delimiters inside so math reads as
    math rather than as inline code; `strip_inline_markup` (read-aloud/titles)
    drops the delimiters and keeps the expression as plain text, without
    re-parsing it as markdown (so `a_b` stays `a_b`, not italic). New pure
    `detect_math` helper with strict, currency-safe delimiter rules (Pandoc-
    style, slightly stricter): inline open `$` must be followed by a non-space
    AND non-digit char, close `$` must be preceded by a non-space char and not
    followed by a digit, no newline crossing; `$$…$$` matched first (the
    doubled delimiter is unambiguous). 9 unit tests incl. the currency cases
    that motivated the strict rules ("$5 to $10", "from $1,000 to $2,000" all
    stay plain) and a multibyte check. Still a pre-render, not typesetting —
    KaTeX integration remains the eventual upgrade.

### Non-HTML document sources (markdown, mermaid, math) — original framing

User framing arc, v0.1.168→v0.1.169:
- "can we do local markdown loading in the web browser?"
- "maybe there is a shared abstraction for webpages that markdown
  hooks into"
- "internal with a way of extracting some things from theme for
  markdown and some animations that could be enabled on the
  markdown... can then be easy to A/B web regressions too"
- "tables we have a lot of button work for quick actions and
  wysiwyg potential here"
- "the other payoff is when we add `[[` and `]]` as link bound
  for local relative paths and urls to jump between web and
  markdown"
- "mermaid was always annoying to reflow or organize on desktop
  and impossible on mobile (and no way to set constraints for
  hardware blocks) and math... no keyboard support anywhere...
  voice might be huge here"

User shared a corpus of their actual markdown notes (RPI course
notes from 2019-2022). What's in them:
- Mermaid blocks **everywhere** — Dijkstra traces with 11 step-
  by-step graphs back-to-back, hardware datapaths, process
  state diagrams. The diagrams are doing real work.
- Wiki-links in four flavors: `[[Bare Name]]`, `[[../relative/path]]`,
  `[[file with extension.pdf]]`, `![[embed.png]]`
- Heavy tables — cache-tag tables with hex values, parity tables,
  MIPS instruction formats. Substance, not decoration.
- Math everywhere: `$O(n+m)$`, `$pre(v)<pre(u)$`, Pandoc-style
  superscript `2^r^`, subscript `10011010~2~`
- Code blocks with meaningful language tags (`pseudocode`, `bash`,
  `mermaid`)
- HTML entities inline (`&mdash;`, `&nbsp;`, `&infin;`)
- Strikethrough `~~revised text~~`
- `[TOC]` directive

User's framings on the heavy types:
- Mermaid: "source was always annoying to reflow." Don't render
  mermaid faithfully — that preserves the annoyance. Parse to a
  native graph model, render with primitives we can pan/reflow/
  constrain. Mermaid becomes an **import format**, not a render
  target.
- Math: "no keyboard support anywhere... voice might be huge."
  Math should be a structured `MathTree`, not a LaTeX string.
  Then voice→math becomes a direct producer of the tree without
  round-tripping through LaTeX text.

This rules out the original "Option A" plan (`pulldown-cmark →
HTML → existing renderer`). It works for text-only markdown, but
the user's actual content is dominated by mermaid + math + tables
+ wiki-links — none of which survive that pipeline cleanly. Option
A becomes "pulldown-cmark plus a mermaid renderer plus a math
renderer plus a wiki-link parser plus a table interaction layer"
at which point it's not 50 lines anyway.

The real plan is "Option B with content-typed nodes," but it's
multi-month and the layout substrate (next section) is the
hardest piece. Filed; not starting yet.

#### The tree (L — multi-month)

- [ ] **L — DocNode tree as the shared abstraction.** Typed enum
  covering: Heading, Paragraph, Link, **WikiLink** (resolves to
  internal nav), List, Quote, **CodeBlock(lang)**, Image,
  Strikethrough, Bold, Italic, **Table { headers, rows, alignment }**,
  **Diagram { nodes, edges, subgraphs, layout_hint }**,
  **Math(MathTree)**.

  Inputs (each separately shippable):
    - HTML walker (refactor of existing `qml_gen`)
    - Markdown walker (pulldown-cmark + wiki-link pre-pass)
    - Mermaid parser → `DocNode::Diagram` (graph-aware)
    - LaTeX parser → `DocNode::Math` (for round-tripping notes)
    - *Future*: voice → `DocNode::Math`, voice → `DocNode::Diagram`

  Theme contract: renderer queries `theme.style_for(NodeKind)`
  rather than hardcoding tag→style mapping. Both HTML and
  markdown get the same answers.

  **A/B regression payoff:** canned DocNode tree as fixture,
  diff generated QML across versions. Catches renderer
  regressions separated from parser regressions. This is the
  primary architectural justification — three independent reasons
  point at this refactor (theme-extracted rendering, A/B testing,
  unified wiki-link namespace).

  Don't start without committing to the multi-month arc. Smaller
  intermediate ships should *build vocabulary* the DocNode tree
  will eventually use (see RenderTier work, below) without
  pre-committing to the whole refactor.

#### Native diagram primitive (L — depends on layout engine, below)

- [ ] **L — `DocNode::Diagram` as a graph-shaped layout, not a
  mermaid renderer.** Nodes have optional position pinning. Edges
  have routing strategies (straight, orthogonal, curved). Subgraphs
  carry grouping constraints ("hardware block, don't separate").
  User can drag-to-reorganize, pinch-to-zoom, snap-to-grid.

  Mermaid source is one input. Hand-drawn graphs (future) are
  another. CSS pattern detection on web pages (cards in a grid,
  comparison tables) is another — same engine, different parser.

  User framing: "mermaid like where pinning is optional and reflow
  is more intelligent and then this can be used on webpages too
  for some more css patterns... mermaid being the target because
  it is more performance focused than the general web... although
  maybe the web is now with big site-builders like Figma."

  Key insight: mermaid wins as the model because the source already
  declares structure. Web pages mostly hide structure inside CSS
  hacks. But Figma/Webflow-generated sites increasingly produce
  structurally graph-shaped HTML, so the gap is closing.

#### Voice-to-math, voice-to-diagram (L — depends on tree)

- [ ] **L — Spoken math → MathTree.** Whisper produces text;
  parser produces structured math. "x squared plus the integral
  from zero to infinity of f of t dt" → MathTree → render. Prior
  art: MathTalk, AsTeR (research-grade, dated). Architectural seam
  matters: tree, not LaTeX string. Otherwise voice round-trips
  through text and loses the benefit.

- [ ] **L — Spoken diagram → Diagram.** "Flowchart with three
  nodes: input, process, output, arrows from input to process to
  output" → graph structure → layout engine. Same architectural
  point as math.

### Layout engine (filed for later, guiding immediate work)

User direction v0.1.169: "I want to file it for later but use it
to guide the immediate: fix frame drop on scrolling... build out
some basic layout stuff now and learn from this data I provided
as well as general web practices."

The destination: a general-purpose graph layout engine that takes
(nodes, edges, grouping constraints, viewport) and produces
(positions, edge paths, drag update protocol). Force-directed
with constraint hints is the leaning architecture — generalizes
across mermaid, hand-drawn graphs, and CSS pattern detection.

Hard problems acknowledged:
  - Layout algorithms have decades of literature (Fruchterman-
    Reingold, Sugiyama-style hierarchical, Cassowary constraint
    solving). Each is multi-thousand-line in reference impls.
  - 60fps drag-with-reflow requires *incremental* layout, not
    from-scratch each frame. Rules out "just call Graphviz."
  - The CSS-pattern-detection side is its own subproject — a
    "shape recognizer" that extends `web_islands.rs` /
    `layout_detect.rs` with richer output.

- [ ] **L (3-6 months) — Layout engine, built from scratch.** User
  preference: build vs. wrap, to be confirmed but leaning build.
  Milestones: force-directed v1 → grouping constraints → drag-
  reflow → CSS pattern application.

  Don't start without explicit go-ahead. The vocabulary built in
  RenderTier work below should be the vocabulary this engine
  consumes.

### RenderTier — the immediate work (was "fix scroll perf")

User insight v0.1.169: "seems like long pages and subsection
rendering are unrelated but if you can make them relate at a
deeper level that might be perf gain."

The unification: long-page scroll-frame-drops and subsection
rendering (`<details>`, SlidePopup, file-picker drawer, settings
drawer) are the same problem — *deciding what to compute right
now versus later*. They currently solve it with different
machinery (`Loader { asynchronous: true }` for the first,
`visible: parent.expanded` for the second), and **neither defers
binding evaluation, which is where scroll frame drops actually
live**.

The shared abstraction:

```rust
enum RenderTier {
    Paid,    // bound, painted, hit-testable — the element in front of you
    Dormant, // instantiated, geometry known, volatile bindings paused
    Latent,  // description exists, no QML instance, no bindings
}
```

The classifier decides per-element. v1 is dumb: viewport proximity
+ scroll velocity drives the tier. >2000px away = Latent;
±2000-±500px = Dormant; within 500px = Paid. That alone eliminates
80% of per-frame binding work on long pages AND lets collapsed
details defer until tapped.

The deeper win (the one the user anticipated): the *classifier*
becomes the shared layer. Scroll perf and subsection deferral
both go through one decision-maker. The future layout engine
plugs in here naturally — "this subgraph is dormant" is exactly
what a force-directed solver needs to know which groups to skip
in iteration.

Implementation plan (per-ship increments):
  - [ ] **M — v0.1.169: introduce `RenderTier` + classifier
    skeleton + apply to qml_gen emitted elements above/below the
    viewport.** Smallest version of itself. No retrofit of
    `<details>` or SlidePopup yet. Just: emit a `tier` property
    on each element, bind it to a classifier driven by contentY
    and element geometry, gate the expensive bindings on
    `tier === RenderTier.Paid`. Measure scroll fps on the user's
    test page before/after. If no improvement, the bottleneck
    isn't where we think it is and the abstraction doesn't pay
    its way yet.
  - [ ] **S — Retrofit `<details>` to use RenderTier.** The
    `_d{uid}.expanded` binding from v0.1.168 stays — it's the
    trigger, not the doer. Expanding promotes children to Paid;
    collapsing demotes to Latent.
  - [ ] **S — Retrofit SlidePopup.** Currently uses `visible: ...`
    + z-order + Behavior gating. Tier-driven instead.
  - [ ] **S — Retrofit file-picker drawer + settings drawer.**
  - [ ] **M — Smarter classifier.** Scroll velocity, hover
    proximity, recent-interaction history. Promotes elements
    *before* they enter the viewport during a fast scroll.
  - [ ] **M — Tier-aware binding generation in qml_gen.** The
    biggest win: emit only the *cheap* bindings at Latent tier,
    upgrade to expensive bindings on promotion. E.g. `width:
    parent.width` is always paid; `font.pixelSize: <calc>`
    upgrades only when Paid. Saves binding eval cost at every
    tier.

The discipline: every ship must fix a *measurable* scroll issue
OR be a free side-effect of a fix already being done. No "I'll
add this abstraction now because we'll need it later" — that's
the multi-month layout-engine arc, which is filed.

### v0.1.169 user measurement feedback

User tested v0.1.169 on Apple.com iPhone page: "it's still slow
on apple.com and I think it might be the text just being so
much and not being cached the right way."

The hypothesis: text rendering is the cost, and Qt's text-layout
cache isn't surviving across scrolls. Plausible — Apple pages
have dense typography with many Text elements, each with their
own font shaping work. If wrapMode triggers re-layout on every
opacity transition (it shouldn't, but might), that'd manifest
exactly as "slow text scroll."

Don't act on this hypothesis yet — v0.1.170's profiling
infrastructure exists precisely so we don't guess. Once user
captures a profiling.log of an Apple.com scroll session, the
fps_p and dt_p percentiles will tell us:
  - If frame deltas are dominated by individual 80-200ms outliers
    → text shaping or image decode (not scroll-machinery issue)
  - If frame deltas are uniformly 20-30ms (~30-50fps) → likely
    binding eval or compositor (RenderTier hypotheses)
  - If frame deltas are tiny but touches lag → input pipeline
    (different category entirely)

Filed for the next measurement-driven ship.

### Image lifecycle (next perf ship — v0.1.172 measurement confirms)

User hypothesis after seeing v0.1.169 ship: "hmm maybe images
are causing it [on Apple iPhone page]."

**v0.1.172 measurement (user observation):** modal-skip
removed the catastrophic 175ms hitches but exposed the next
layer of cost. Phone-specific observations:
  - MacBook iPhone page: loads fine, hitches gone
  - Phone iPhone page: hitches gone but *longer sustained
    stutters*; footer "consistent but slightly lower
    frequency stutter"
  - Critical user observation: "iphone page I think loads
    images offscreen" — directly confirms eager-decode
    model. Qt is decoding everything regardless of viewport.

**v0.1.173 measurement (laptop, after _sourceArmed shipped):**
  - Apple iPhone page on laptop: median 111-125fps, max frame
    22ms, no catastrophic outliers. Clear improvement vs
    v0.1.170 (which had 166ms page-transition hitch). The
    sustained stutters that v0.1.172 exposed are GONE on the
    laptop.
  - MacBook-pro, wikihow, alx.sh/l/asahi-dev pages all run
    clean throughout.

**v0.1.173 measurement (phone) — modest, not a fix:**
  - Worst-case hitch: 175ms (v0.1.170) → 89ms (v0.1.173).
    Halved. Real improvement on the catastrophic-frame
    cost from below-the-fold image decode bursts.
  - Sustained slow region: ~unchanged. p25 still drops to
    32-37fps during scroll. v0.1.173 did not fix the
    dominant phone cost.
  - Likely diagnosis: scroll-time cost is per-frame layout
    work (iPhone hero is complex) + image decodes as
    content enters the read-ahead zone during scroll.
    Lazy deferral only helped the *initial-load* burst,
    not the steady-state scroll cost.

**CRITICAL (v0.1.173 user observation): hero section is
not rendering correctly.** User: "the hero section does
not show up very well inside my browser... that page is a
bunch of empty space and occasional text." This is a
*rendering gap*, not a perf issue. Performance numbers
are meaningless if the page being rendered isn't showing
what the user came for.

  Hypotheses (need diagnosis):
    1. v0.1.172 `is_hidden()` over-matches `opacity: 0`
       set as CSS-transition starting state (apple uses
       `opacity: 0 → 1` animations on hero load).
    2. Apple hero relies on JavaScript we don't execute;
       HTML is skeleton, populated by JS.
    3. Apple hero uses CSS `background-image` (in `style=`
       attribute) rather than `<img>` tags; our renderer
       only handles `<img>`.
    4. v0.1.173 `_sourceArmed` over-defers — wrappers at
       y > 1500 stay un-armed despite onLoaded widening
       the viewport (would require closer look at the
       binding-re-evaluation flow).

  Diagnostic test the user can run: `WEB2QML_LAZY_IMAGES=0`
  on the iPhone page. If hero now shows → v0.1.173 broke
  it. If hero still empty → bug predates v0.1.173 and we've
  been measuring perf on a page we never actually rendered
  correctly. Different fixes either way.

  Until diagnosis: hold the v0.1.173 ship in "shipped but
  rendering correctness gap pending" state. Default-on may
  need to be reconsidered if hypothesis 4 is correct.

Diagnosis: modal-skip removed worst case (modals had
images-that-triggered-decode-hitches), exposed ordinary
image-decode-during-scroll on a memory-tight phone. Phone
RAM/GPU pressure is the next bottleneck.

The Apple iPhone marketing page has ~50+ high-res JPEGs in
nested horizontal-scroll galleries. RenderTier v1 gates `opacity`,
which lets Qt skip *paint composition*, but does NOT:
  - free decoded image memory
  - skip an in-flight image decode
  - skip texture upload to GPU
  - skip image cache thrash if cache size < total page image bytes

- [ ] **M — Hero-section initial decode cost (v0.1.173
  remaining bottleneck).** User v0.1.173 observation: "the
  iphone header is now the most significant stutter...
  stutter goes away entirely actually once the page scrolls
  a bit." Below-the-fold lazy-source deferral worked;
  above-the-fold hero is now the dominant cost.

  The Apple iPhone hero contains: rotating image gallery
  with multiple high-res JPEGs, video-poster elements,
  oversized navigation thumbnails for subsequent gallery
  items. Other Apple product pages have similar hero
  structure but stutter less, suggesting the iPhone hero
  has something specific.

  Three sub-investigations:
    1. **Image-count cap in initial viewport.** Can we limit
       the number of images that decode in the first frame
       to, say, 3? Subsequent ones defer to next-frame even
       if they're "above the fold." Risk: visible "loading"
       on hero scroll.
    2. **Aggressive sourceSize clamping in hero region.**
       Apple's hero serves 2x-3x retina; if we clamp
       sourceSize more tightly for elements in the first
       ~800px, decode time drops. Could be heuristic: when
       multiple images are within initial viewport, clamp
       all of them harder.
    3. **Per-element layout cost investigation.** The hero
       has complex CSS we don't render natively; some of
       its elements may be hitting expensive QML constructs
       on first paint. Worth inspecting the DOM and the
       emitted QML for the specific hero region.

  Don't pursue without measuring whether (a) the laptop
  observation generalizes to phone, and (b) the
  QQuickAsyncImageProvider approach (filed below) would
  subsume some of these wins via cache+decode control.

- [ ] **M — Image lifecycle: defer-decode-until-Paid, not
  release-on-Latent.** Skia comparison (v0.1.172 user
  question): Skia's model is lazy decode + cache result +
  global eviction. Decoded bytes live in a 96MB GPU cache
  by default. Decode is deferred to first draw, not triggered
  by image creation. Qt's QQuickImage decodes eagerly when
  `source` is set (asynchronous just moves it to a worker
  thread, doesn't defer to first paint). Qt's pixmap cache
  is ~10MB default, ~10x smaller than Skia's, so image-heavy
  pages thrash.

  The right shape is NOT "release source when in Latent tier"
  (causes re-decode thrashing on scroll-back). The right
  shape is **defer source until first promotion to Paid**,
  then trust Qt's cache. Off-screen images never decode;
  on-screen images decode once and stay decoded.

  Implementation:
    - Wrapper Item from v0.1.169 already knows the tier.
      Add a `_sourceArmed` sticky property: starts false,
      flips to true on first non-Latent observation, stays
      true forever after.
    - Image source binding: `_sourceArmed_<uid> ? "<url>" : ""`.
    - Initial render: Image elements present in DOM but with
      empty source → no decode, no GPU upload, no cache cost.
    - Scroll brings an Image into the Paid zone → wrapper
      becomes non-Latent → _sourceArmed flips → source binding
      re-evaluates to the URL → Qt fetches/decodes.
    - Scroll back later → wrapper goes Latent → opacity goes
      to 0 (Qt skips paint) → source stays armed (no
      re-decode needed if Qt's cache still holds bytes).

- [ ] **M — Increase Qt's pixmap cache limit.** Default ~10MB
  is too small for image-heavy pages. Push to 64-96MB
  (matching Skia's default) to let Qt hold all recently-shown
  images without thrashing. Investigation needed: is
  QtQuick's image cache the same as
  `QPixmapCache::setCacheLimit()` or separate? Qt docs unclear.

- [ ] **L — YUV-to-GPU bypass for JPEG (research only).**
  Skia does this for JPEG perf wins. Qt doesn't expose it.
  Would require dropping below QQuickImage to a custom
  QQuickItem with QSGTextureProvider. Substantial. File for
  consideration; don't pursue without measurement showing
  CPU decode is the actual bottleneck (vs cache thrash,
  which the previous filing addresses).

- ✅ **`asynchronous: true` and `cache: true` audit on
  emitted Image elements** — verified v0.1.374. Audited all
  Image-emission sites in qml_gen (img and picture tag
  handlers, both eager and lazy paths) and manpages renderer.
  Every site emits `asynchronous: true\ncache: true`. No
  changes needed; audit-only closure.

- [ ] **M — Custom QQuickAsyncImageProvider with our own
  cache.** v0.1.172 user question: "is there another Qt
  canvas or element that can better fit the Skia model?"
  Answer is yes — `QQuickAsyncImageProvider` is Qt's
  official extension point for taking over image loading
  globally. Register a `web2qml://image/<encoded-url>` URL
  scheme; `qml_gen` rewrites Image sources to use it; the
  provider on the Rust side decides:
    - When to fetch (defer until first request)
    - What cache holds the result (our own, sized like
      Skia's ~64-96MB)
    - Eviction policy (LRU first, tier-aware later)
    - Decode threadpool
    - Whether to return a `QQuickTextureFactory` for
      direct-to-GPU upload (skips one CPU copy)

  Migration is clean: qml_gen URL rewrite is one-line; all
  perf work is in Rust. Existing Rust HTTP fetch feeds
  directly into the provider — bypasses Qt's network stack.

  Caveat: docs don't make hard timing promises for when the
  provider is invoked for off-screen images. Possibly Qt
  invokes eagerly on Image instantiation regardless of
  viewport, in which case we get cache control but not lazy
  fetch. **Build a small test before committing** to this
  architecture: instantiate a hidden Image, verify the
  provider isn't called until visible. If eager, combine
  with the `_sourceArmed` pattern above — `_sourceArmed`
  defers the URL assignment, the provider defers the
  decode-and-cache once the URL is assigned.

  Two-layer architecture (if both are needed):
    - `_sourceArmed` in QML: defers `source` until first
      promotion to Paid. Off-screen Images stay sourceless.
    - QQuickAsyncImageProvider in Rust: when source IS set,
      controls fetch/decode/cache/eviction with full
      Skia-like behavior.
  Together this matches Skia's model on Qt's stack with
  minimal architecture changes. The `_sourceArmed` ship is
  the cheap one (pure QML, no new C++); the provider ship
  is the deep one (Rust+Qt integration, ~few hundred lines).

  **Unified substrate property (v0.1.172 user direction).**
  User asked: "are there any that help us with web previews
  in the future or graph previews or file previews?" Yes —
  one custom provider can serve many call sites, each with
  its own URL scheme registered to the same provider impl:
    - `web2qml://image/<encoded-http-url>` — HTML img tags
      from rendered pages (the primary perf case)
    - `web2qml://preview/<encoded-page-url>` — web link
      previews (thumbnail of a page, generated on demand)
    - `web2qml://file/<encoded-path>` — file previews
      (image/PDF/text thumbnails in file picker)
    - `web2qml://diagram/<id>` — rendered SVG/mermaid
      diagrams from the future native diagram primitive
    - `web2qml://generated/<hash>` — LLM-generated visual
      hints, icons, glyphs
  All share: one decode threadpool, one cache, one eviction
  policy. Provider dispatches on URL scheme. Build the
  provider once for HTML images; later call sites get
  Skia-like caching for free as we register their schemes.
  Cross-call-site sharing: if the same image URL is
  referenced by both an HTML page and a link preview, the
  decoded bytes live in one cache entry, served to both.

- [ ] **L — Full custom QQuickItem with QSGTextureProvider +
  cross-page batch sharing.** Last-resort option if
  QQuickAsyncImageProvider's invocation timing is wrong for
  our needs OR if we want JPEG YUV→GPU bypass. Bypasses
  QQuickImage entirely; we manage the scene-graph node
  directly.

  Additional capability over Option 2: **batch image sharing
  across page lifetimes.** v0.1.172 user observation:
  navigating between Apple subpages (iPhone → iPhone Pro →
  iPhone Air) re-decodes shared assets each time. A cache at
  the QSGTextureProvider layer can survive page navigation
  and serve same-URL textures across navigations. Substantial
  memory win for mobile, where decoded RGBA bytes are the
  dominant heap pressure.

  Substantial work (rewriting QQuickImage). Don't pursue
  without measurement showing the QQuickAsyncImageProvider
  approach has hit a ceiling. The batch-sharing-across-pages
  property COULD be partially achieved with Option 2 by
  giving the provider's cache a longer lifetime than the page
  — investigate that first.

- [x] ~~**S — Refactor: extract image emission helper.**~~ DONE v0.1.871.
  Extracted the duplicated ~30-line Column>Image(+alt Text) emission (the
  img-tag handler and the picture-element source path were byte-identical)
  into `emit_image(out, r, alt, attr_w, attr_h, sty)` in qml_gen.rs. Both
  sites now call it; output is unchanged (verbatim copy). 133 qml_gen tests
  pass. Tiny-image skip stays in the callers (they guard before allocating).

### Lazy-load patterns — generic extraction (v0.1.174 conversation)

User v0.1.174 question: "do they all coincide to the same
treatment on our end or is there really a difference in the
varying structures? I don't want to chase all the specifics
of each site and I think some way of just knowing that images
are here and present themselves is a better target."

Honest answer: there are real structural differences (which
*attribute* holds the URL varies), but they all converge on
the same emit-an-Image-with-this-URL output. The variety is
in URL location, not in rendering. The right abstraction is a
generic `extract_image_url(element)` that tries known
locations and returns the first viable URL.

Patterns identified (in order of priority):
  1. `<img>` direct: `src`, `data-src`, `data-lazy-src`,
     best `srcset` (already partly handled)
  2. Parent `<picture>` `<source srcset>` excluding
     `data-empty`/`data:` URIs (handled v0.1.174, but
     specific to picture handler)
  3. Sibling `<noscript>` block with same-named picture
     (not handled — progressive enhancement fallback)
  4. Inline CSS `background-image: url(...)` on parent div
     (not handled — common on hero sections that use
     divs instead of img tags)
  5. SVG `<image href>` (not handled)
  6. Canvas/WebGL (unrenderable without JS runtime)

- [x] ~~**M — Generic `extract_image_url` function.**~~ Done v0.1.889.
  New `extract_image_url(el)` in qml_gen.rs consolidates the lazy-load
  resolution the img and picture handlers carried separately: (1) the
  element's own src-like attrs in priority order (`src`, `data-src`,
  `data-lazy-src`, `data-original`, `data-srcset`, `srcset`), first
  non-placeholder wins; (2) failing that, `<source>` descendants (picture
  pattern) — skip `data-empty`/`data:` srcsets, keep the last viable. Both
  handlers now call it; new lazy-load patterns get added in one place.
  Proven behavior-preserving: the byte-for-byte golden harness (img via
  article.html, picture via media.html) is identical before/after. 2 new
  unit tests cover the candidate priority and the picture source scan.
  (Minor, documented: for the pathological case of a `<picture>` carrying
  its own src-like attr, candidates now win over its sources — can't occur
  with valid markup, and is arguably more correct for lazy-loaded pictures.)

- [ ] **S→M — Honor `<source media>` queries for picture sizing.**
  REASSESSED v0.1.887: not a clean S. The premise assumes a render width
  is known at generation time, but qml_gen is width-agnostic — width is
  resolved at QML *runtime* via `parent.width` (there's no viewport-width
  value threaded into generation). So matching `media=(max-width:734px)`
  against "our render width" can't happen at emit time. The correct fix
  is one of: (a) emit a runtime source-selection binding keyed on
  `parent.width` (proper CSS `<picture>` semantics: first source whose
  media matches), or (b) plumb a target/pane width into qml_gen and select
  at generation time. Both are M. The current "pick last viable" is the
  Apple small→large convention and stays correct-but-largest until then.
  Note the right selection rule is *first matching source in document
  order*, not last.

- ✅ **`<noscript>` fallback parsing for picture/img** —
  addressed v0.1.374. Originally framed as "parse noscript
  blocks for last-resort URLs"; turns out the existing
  noscript handler already renders inner content, so any
  noscript-wrapped `<img>` was already finding its way to
  output. The actual remaining gap was placeholder
  detection on the OUTER `<img>` (sites use a base64 1x1
  png placeholder + sibling noscript fallback; the
  placeholder rendered before noscript got a chance).
  v0.1.374 expanded the placeholder filter (new module-
  level `qml_gen::is_placeholder`) and the source-
  resolution candidate list to include `data-original` and
  `data-srcset`. 8 unit tests.

- [x] ~~**M — CSS `background-image: url(...)` extraction.**~~ Done
  v0.1.887 (v1 scope). New pure `extract_bg_image_url(style)` in
  qml_gen.rs parses `background-image`/`background`-shorthand `url(...)`,
  stripping quotes/whitespace and rejecting `data:` URIs, the `none`
  keyword, and gradients (no `url()` token); it requires a `background`
  property so `url()` in `cursor`/`mask`/`list-style-image` isn't grabbed.
  Wired into the generic-element emitter, but deliberately scoped to the
  `parts.is_empty()` arm — a leaf element with a background image and no
  rendering children, which is exactly the "currently invisible" case.
  That scoping means (a) no `<img>` child to double up with and (b) hero
  sections that already render text/children are left untouched, so it's
  strictly additive. Such a leaf now emits an `Image` (placeholder URLs
  filtered by `is_placeholder`). 4 unit tests incl. quotes, shorthand,
  rejections, and a multibyte-URL char-boundary check. The position/size/
  repeat caveat stands — image shows at the emitter's default width.
  Broadening to non-leaf elements (background + overlaid content) is a
  future call once it can be visually verified.

### Rendering fidelity arc (v0.1.174 strategic question)

User v0.1.174 question: "how is css and html able to capture
so much variety in the first place and what strengths and
weaknesses can we build on?"

The variety comes from three properties of HTML+CSS:
  1. **Separation of structure and presentation** (one
     HTML pattern → infinite visual variations via CSS)
  2. **Layered layout systems** (block flow, flex, grid,
     absolute, transforms — composable per element)
  3. **Permissive substrate** (undefined attributes
     ignored, letting authors extend without breakage)

Our strengths:
  - Skip what we can't do gracefully (hidden modals v0.1.172)
  - QML's layout engine is fast and battle-tested
  - Rust backend can preprocess HTML before emission
  - We control the destination format, not bound to web specs

Our weaknesses:
  - No CSS cascade or inheritance (text doesn't inherit
    parent color)
  - No media queries (one rendering for all devices)
  - No JS runtime (lazy-load, interactions, animations dead)
  - No CSS state (`:hover`, `:checked`, `:focus`)
  - We patch per-site instead of recognizing patterns

Strategic tiers for future investment:

  **Tier A — generic attribute extraction (where we are
  now, just more thorough).** The image-pattern filings
  above. Plus CSS inheritance for a small property set.
  Tractable, incremental wins.

  **Tier B — design-system pattern recognition.**
  Recognize "card," "hero," "gallery," "modal," "nav"
  class patterns and emit semantic QML directly. Apple
  provides hints via `data-component-list="Card Gallery"`
  etc. Bigger bet but higher reward — skips the deeply-
  nested div soup, emits direct semantic equivalents.

  **Tier C — selective JS shims.** Execute the minimum
  JS needed to handle common lazy-load idioms. Not a full
  runtime; just `window.__lp?.(event)` and similar.

  **Tier D — give up gracefully.** Accept rendering gaps
  on showcase/portfolio sites. Show content without the
  animation.

- [ ] **M — CSS inheritance for cascade-relevant properties.**
  Properties that should inherit from parent: `color`,
  `font-family`, `font-size`, `font-weight`, `line-height`,
  `text-align`. When emitting QML for any element, look up
  the cascade chain for these properties if not directly set.
  Substantially improves text rendering on sites that set
  these once on body or container. Filed as Tier A.

- [ ] **L — Design-system pattern recognition.** Recognize
  common class-name + structure combos and emit semantic
  QML instead of literal div translation. E.g. `class="card
  tile feature-card"` with `<picture>` + `<h3>` + `<p>` +
  `<button>` → emit a clean QML card primitive. Reduces
  emitted QML size, improves layout quality, easier to
  optimize per pattern. The future "layout engine" arc
  composes here. Filed as Tier B.

- [ ] **L — Selective JS shims for lazy-load idioms.**
  Tiny interpreter that runs only the `window.__lp` /
  `IntersectionObserver` / `data-src→src` swap patterns
  common in lazy-load implementations. Not a full JS
  runtime. Filed as Tier C — last resort, only if A and B
  don't cover enough of what users actually visit.

### Hidden-modal content (v0.1.171 user observation)

User pasted Apple footer DOM showing eight+ nested
`<div data-component-list="Modal" aria-modal="true">` subtrees
in the footer region — each is a full modal dialog (header,
viewport-content, paragraphs of legal text, close button)
that the live site keeps invisible via CSS (`opacity: 0`,
`pointer-events: none`) until the user clicks a `+` button on
a feature card. On the live site these layouts are computed
but invisible; users don't see the cost.

Our renderer doesn't know `aria-modal="true"` means "this is
collapsed initially." It lays out every modal subtree as if
visible. The Apple iPhone page has at least 8 such modals
just in the footer (trade-in, getting-started, designed-to-
last, ios-apple-intelligence, privacy, advanced-cameras,
environment, safety), each containing paragraphs + headers
+ images.

User observation correlating with profiling.log analysis:
"apple's footer is where the screen stutters a bit extra...
seems like lots of `aria-modals` and nested paragraphs."
This is a *separate* perf contributor from image decode
hitches — modals are layout work, images are decode work,
both cost real frames.

- [x] ~~**S — Skip emission of `aria-modal="true"` subtrees by
  default.**~~ Already done (v0.1.172), verified this pass. `is_hidden`
  (qml_gen.rs) returns true for `aria-modal="true"` without an `open`
  attr, and for inline `display:none` / `visibility:hidden` / `opacity:0`
  (the opacity check guards against `0.5`/`0.1` false-positives). The
  caller skips the whole subtree (returns `""`) rather than emitting a
  placeholder marker — skipping is cleaner than a marker and matches the
  intent. No code change needed.
- [ ] **M — Inline-style visibility heuristics in `qml_gen`.**
  Currently we read `style` attribute for some hints but
  not all. Add a pre-walk that resolves `display: none`,
  `visibility: hidden`, `opacity: 0` from inline styles
  and treats those subtrees the same as `aria-modal`
  collapsed dialogs.
- [ ] **L — Real dialog/modal interaction.** Once we skip
  modal emission, we lose the user's ability to actually
  open them. Long-arc fix: emit a *button placeholder* for
  each modal trigger (`+` icon on feature cards) that, when
  tapped, reveals the corresponding modal subtree. Requires
  the renderer to retain the modal HTML somewhere
  (compressed, lazy-loaded) so it can be re-rendered on
  demand. Composable with the future layout-engine arc.

### Analytics attribute strip + harvest

User insight v0.1.169 from the apple.com paste: `data-analytics-*`
and `data-component-list` attributes everywhere. They're
tracking/animation hooks, but they're ALSO author-provided
semantic region labels (`data-analytics-section-engagement=
"name:switch to iphone"`, `data-analytics-gallery-id="switch to
iphone gallery"`). Two opposite directions:

- ✅ **Strip `data-analytics-*`, `data-component-list`,
  `data-staggered-item`, `data-anim-scroll-group`,
  `data-focus-expression` from HTML pre-parse** — landed
  v0.1.379. New `qml_gen::strip_tracking_attrs(html)` runs
  BEFORE `scraper::Html::parse_document` in the main fetch
  path. Conservative byte-scanner (no regex) that respects
  attribute boundaries — `data-` appearing inside a quoted
  value or inside text content is NOT stripped. Covers
  prefix matches (`data-analytics-*`, `data-tracking-*`,
  `data-gtm-*`, `data-ga-*`, `data-pixel-*`,
  `data-segment-*`) and exact matches
  (`data-staggered-item`, `data-anim-scroll-group`,
  `data-focus-expression`, `data-component-list`,
  `data-impression-id`).
  Preserves `data-src`, `data-lazy-src`, `data-original`,
  `data-srcset` (real image sources), `data-href` (used by
  detect_js_href), `data-aos`, `data-animate` (used by
  anim_detect), `data-empty` (Apple picture handler),
  `data-bg-image`, `data-background`. Both quoted (`"..."`
  and `'...'`) and unquoted values handled. Idempotent.
  10 unit tests covering positive/negative cases including
  the URL-query-string boundary case (`?data-analytics-id=x`
  inside a URL must NOT be stripped).
- [ ] **L — Harvest those attributes as layout hints BEFORE
  stripping.** Author-labeled region semantics ("this is a
  gallery," "this is a card grid," "this is a paddlenav") feed
  the future shape-recognizer. Higher-signal than CSS class
  inference. Applies to the same future layout-engine arc.
  Specifically useful for big-builder sites (Apple, Stripe,
  Vercel, Linear, Figma exports).

### Pattern harvest corpus

User question v0.1.169: "are there any of those amazing webpages
lists you could also pull from to get patterns?"

Tiered by usefulness for OUR use case (static renderer, not full
JS execution):

**Tier C — actual workhorses (highest value):**
  - Wikipedia (long articles, tables, infoboxes, footnotes)
  - Hacker News, lobste.rs (list-of-items, threaded comments)
  - GitHub repo pages (tab nav, file tree, README)
  - Stack Overflow (Q&A, answers, code blocks)
  - old.reddit.com (comments hierarchy, much better than new)
  - News: NYT, BBC, Ars Technica (article + sidebar + comments)
  - Docs: docs.anthropic.com, docs.stripe.com, MDN, supabase

**Tier A — polished structurally-sane pattern sources:**
  - Apple product pages (semantic data-attributes)
  - Stripe.com (clean structural HTML)
  - Linear.app, Vercel.com (modern minimalist marketing)
  - Modern Webflow/Figma-published sites

**Tier B — stress tests, not pattern sources:**
  - Awwwards Site-of-the-Day picks (WebGL/canvas heavy)
  - Bruno Simon-style 3D portfolios — what we CAN'T render
  - These exist to test graceful degradation, not to copy from

Filed as ongoing background: every ship can pick one Tier C
page as its measurement target. If frames hold on Tier C, ship
is good; Tier A is the next stretch; Tier B is filed under
"degrade gracefully, don't pretend."

- [ ] **M — Rendering-capability-gap detector.** When a page is
  predominantly canvas/WebGL/three.js (script tag analysis +
  empty body heuristic), surface a "this page is a 3D
  experience our renderer doesn't support" message + show the
  best text-only fallback we can produce (title, meta
  description, link to open in system browser). Beats showing
  an empty page or a garbled DOM.

### Language coverage

- [x] ~~**M — German parser.**~~ DONE v0.1.873. `src/voice_intent_de.rs`
  (`parse_de`), same shape as the Romance parsers. Handles two
  German-specific wrinkles: separable verbs (`mach X auf` → open X) and
  umlaut variation (umlaut form + ASCII fallback ö→oe/ü→ue/ä→ae/ß→ss for
  high-frequency commands, since Whisper can emit either). Plus `schau X`
  / `schau dir X an` → cd. Wired ("de"). 10 unit tests pass (incl. the
  `geh zurück` vs `geh zu ` disambiguation and ASCII-umlaut fallbacks).
- [ ] **M — Japanese parser.** Harder due to kanji/hiragana mix in
  whisper output and SOV order. May need more sophisticated parsing
  than the keyword-prefix approach.
- [ ] **M — Mandarin parser.** Similar concerns as Japanese.
- [x] ~~**M — Italian parser.**~~ DONE v0.1.872. `src/voice_intent_it.rs`
  (`parse_it`), modeled on voice_intent_es. Nav/scroll/history/quit/open/
  search + politeness-prefix and trailing-softener stripping + "guarda X"
  → cd. Wired into `voice_lang::parse_for_lang` ("it"); "it" already in the
  Whisper code list. 8 unit tests pass (incl. "vai avanti" vs "vai a "
  open-prefix disambiguation, and bare "ciao" ≠ Quit).
- [x] ~~**M — Portuguese parser.**~~ DONE v0.1.872. `src/voice_intent_pt.rs`
  (`parse_pt`), same shape with BR/EU keyword coverage + accent-stripped
  variants + "olha X" → cd. Wired ("pt"). 8 unit tests pass (incl. "ir para
  frente" vs "ir para " disambiguation).

### Static site interactivity

- [ ] **M — CSS-driven interactivity.** alx.sh-style sites use CSS
  selectors and attribute-driven state. We render statically. Make
  `:checked`, `:hover`, `:target` etc work in the generated QML.
  Substantial because each pseudo-class needs binding logic.

- [x] ~~**TO FILE (user, this session) — Some links are actually
  JavaScript.**~~ DONE v0.1.875. `qml_gen.rs`: new `is_dead_href`
  (pure, tested) flags `javascript:`, bare `#`, `about:blank`, and
  empty hrefs; a real in-page anchor (`#section`) is NOT flagged. In
  the `<a>` handler, a dead href first tries `detect_js_href` to
  recover a real URL from `data-href`/`onclick="...location=..."`
  (navigable if found); otherwise `emit_interactive_cue` renders a
  non-navigable label in the muted `border_color` + a ⚡ marker
  instead of the old silent dead link. 6 unit tests cover detection,
  the cue, recovery, and that real/`#section` links still navigate.
  (Path 2's `href="#"`+onclick recovery already existed via
  detect_js_href; this adds the cue for the unrecoverable case.)
  Note: `<button>` without a recoverable target already renders as a
  styled non-navigable button, so no change needed there.

- [ ] **TO FILE (user, this session) — Filtering on
  alx.sh/l/asahi-dev still doesn't work.** Previously-reported
  rendering gap on this page is still present. "Filtering"
  likely refers to the page's category/tag filter UI (the
  CSS-driven interactivity that alx.sh uses heavily — checkbox
  inputs styled with `:checked` selectors). Linked to the M item
  above (CSS-driven interactivity). User has reported this page
  specifically multiple times now; it's a worthwhile test target
  once the CSS interactivity work begins. Save the page HTML for
  offline reproducer once accessible. **M, blocked on CSS-driven
  interactivity.**

### Workflows v2

- [ ] **S — Workflow QML UI.** Settings panel showing saved workflows,
  start/stop buttons, list of commands per workflow. Currently
  voice-triggered only.
- [ ] **M — Wait-for-prompt detection during playback.** Currently
  600ms pacing between commands. Detect when shell prompt redraws
  and proceed immediately; otherwise wait longer. Eliminates dead
  air for fast commands and stacking risk for slow ones.
- [ ] **M — Parameter substitution.** "git commit -m $1" template
  with prompt for $1 at playback. Currently workflows are literal.
- [ ] **S — Destructive workflow confirmation.** Workflows containing
  `rm`, `git reset --hard`, etc. re-prompt before running.

### Voice recalibration corpus (v0.1.170 conversation, not started)

User direction across the v0.1.170 conversation refined this
from "voice rollback easter egg" → "interactive bisect" →
"fuzzy testing matrix" → its final form: **a synthetic script
the user reads aloud when voice has drifted, producing fresh
ground-truth (text + audio) that we use to find the snapshot
whose fingerprints still match today's reality.**

Why this is the right shape:
  - User-driven recovery. No technician needed. Even kids can
    do it (read the script aloud).
  - Privacy-preserving. The audio + script stay on device. Only
    "snapshot N matched best" leaves, if anything.
  - Skips UI. Just "read this," produce truth, system finds the
    best match automatically.
  - Works even when voice recognition is broken (the user isn't
    *issuing voice commands* during recalibration; they're
    *reading aloud* for capture).
  - The corpus content is **user-configurable**: a phonetic
    script for power users (dense, fast, designed for diagnostic
    coverage); a favorite book passage or song lyric for kids
    (engaging, repeatable, low cognitive load).

User quote, design north star: "the corpus is a synthetic
script that skips all the UI and lets them read things quickly
and easily in the case of extreme drift or recalibration
needed... I would be the only one willing to sit down and read
the corpus unless it is able to be someone's favorite song or
book somehow."

**Architecture (M-sized first ship):**

1. A *corpus* is a `(text, optional reference audio)` pair,
   stored under `~/.config/web2qml/corpus/<name>/`:
     - `script.txt` — known ground-truth text
     - `reference.opus` — captured baseline audio (first
       successful capture becomes the reference; subsequent
       captures are *compared against this*)
     - `metadata.toml` — owner speaker label, creation date,
       free-form notes
2. Multiple corpora can coexist. Each identified speaker can
   point at their own corpus ("kesslerd-phonetic", "[son]-
   brown-bear", "[wife]-quick-sentences").
3. Voice command "voice isn't working" → app picks the corpus
   for the current identified speaker (or shows a picker if
   unidentified) → displays the script text large and readable
   → records the user reading it → encodes as
   opus-with-chapters (one chapter per script line) → runs
   matrix evaluation against current state + last N
   auto-committed snapshots → reports which snapshot transcribed
   the most script lines correctly → offers restore.

**Auto-commit prerequisite:** the matrix evaluation needs
candidate snapshots to evaluate against. So auto-commit on
voice-state change (the M ship from the previous design
discussion) is a prerequisite. Ship that first OR bundle it
into the same ship.

**Matrix output (text-first, for paste-ability):**

```
              current  -1h    -1d    -3d    -7d    -14d
"sentence 1"    ✗      ✓     ✓     ✓     ✓     ✓
"sentence 2"    ✗      ✓     ✓     ✓     ✓     ✓
"sentence 3"    ✓      ✓     ✓     ✓     ✓     ✓
"sentence 4"    ✗      ✓     ✓     ✓     ✓     ✓
"sentence 5"    ✓      ✓     ✓     ✓     ✓     ✓
ref-audio sim   0.42   0.78  0.81  0.83  0.85  0.84

Best match: snapshot -1h (5/5 sentences, 0.78 reference sim)
Restore? [yes / no]
```

The user sees the *shape* of degradation (e.g. "current breaks
on sentence 1, 2, 4 but not 3, 5") which is more useful for
intuition than a single score.

**Three open questions (need user answer):**

1. **Corpus text default.** What ships for users who haven't
   configured one? Options: (a) a phonetic-coverage script
   we write, kid-friendly, Brown-Bear-style pattern with
   original wording (avoid copyright on actual Brown Bear);
   (b) make it required at install time so there's no default.
   Leaning (a) — user wants recovery, not config.
2. **"Voice isn't working" with no corpus.** Does the trigger
   set up a corpus (wizard flow) or just fail-fast ("set up a
   corpus first in settings")? If (a) above ships a default,
   this question goes away — the default is always usable.
3. **One corpus per identified speaker, or one global?** I
   lean per-speaker — the kid's script is different from the
   parent's. But "global default + per-speaker override" might
   be simpler.

**Encoding format investigation (done, awaiting user confirm):**

User said "no wav please... will be too large too fast... opus
containers with chapters is much better." Checked
Cargo.toml — no opus/ogg/flac deps currently. Options:
  - `audiopus` (safe wrapper around libopus, requires C lib
    on system — `libopus-dev` on Debian/Ubuntu, on Fedora it
    ships in `opus-devel`)
  - `ogg` crate (pure Rust) for container; pair with `opus`
    for the codec
  - Fall back to WAV via `hound` (pure Rust, smallest dep
    delta) for the first ship; revisit if storage becomes a
    problem. Caveat: user explicitly rejected WAV.
  - `symphonia` decodes opus but doesn't encode it.

  Honest assessment: opus encode in Rust requires a C dep no
  matter which crate. Asahi Fedora ships opus-devel, so this
  is a one-time `dnf install` for the user. The dependency
  addition is real but justified by the storage-size win and
  by user's explicit "no wav" stance.

  Plan when building: add `opus` and `ogg` crates, document
  the system-package requirement in the build instructions
  for v0.1.171.

**What gets filed for later (NOT in first ship):**

- Sliding-window pocket-detection in the background. Watches
  recent audio rms / spectral / fingerprint-update-rate. When
  input looks degraded but fingerprints are still updating,
  *quarantine* the updates to a pending-review branch rather
  than applying. Catches the cafe-test failure mode
  structurally. User direction: "sliding window updates in
  behavior can be used to automatically track down users that
  keep a phone in their pocket or we can reset and rely on
  other database sessions to provide negatives while we start
  building new branches and tags for kids every so often."
- Cross-session negative examples. When evaluating a new
  enrollment, score it against OTHER speakers' fingerprints
  from other sessions/branches; if it matches "not the
  intended user" better than "the intended user," reject the
  enrollment.
- Auto-branching when drift exceeds threshold ("voice has
  changed too much from baseline, starting a new branch named
  '<speaker>-2026-q3'"). The kid-yearly-bump case, but
  event-driven not schedule-driven.
- Singing-as-corpus. Engaging for kids but technically harder
  (whisper transcribes singing imperfectly, pitch-hint logic
  needs to disable for sung input, backing music complicates
  capture). Defer until reading-as-corpus is working.
- IT-support framing. The matrix output is *also* what a
  remote technician would see (with user consent), via a
  privacy-preserving export. Raw fingerprints + audio never
  leave the device; only the matrix outcomes + "restore to
  snapshot N" recommendation do. User direction: "if we have
  a hard time with it, normal people will too" — the
  diagnostic interface is mission-critical, not power-user
  optional.
- Visual matrix renderer. Color-coded HTML grid, easier to
  scan at a glance than text. Filed; text version ships
  first because it's what's paste-able in chat for our
  collaboration loop.
- **Opportunistic read-aloud sampling (v0.1.171 user idea).**
  User: "could also add some ux to suggest users to read out
  loud their content on the web if audio is quiet with a popup
  to take opportunistic sampling and future git framework."
  When ambient noise is low AND the user has been reading
  long-form content for >N minutes, prompt: "want to read
  this aloud? helps us recognize your voice better." If yes,
  capture the audio of the current paragraph (or
  user-selected range) and seed a snapshot. Each accepted
  prompt = one git commit in the voice corpus. Builds a
  longitudinal, naturally-diverse corpus without the user
  ever doing a "calibration session."
  
  Better entry point than the "voice isn't working" rescue
  flow because it's *proactive* (good moment, opportunistic)
  vs *reactive* (something broke, fix it). The two compose:
  opportunistic sampling builds healthy corpus, rescue flow
  uses it when needed.
  
  Constraints:
    - Popup MUST be unambiguous about audio-stays-on-device.
    - Debounce hard — at most one prompt per session, only
      after sustained reading on long-form content. Doesn't
      pester.
    - For kid profiles: parent must approve audio capture
      before it associates with a child profile. Could
      default to "sample, hold for parent review, discard
      if not approved within 24h."
    - Detection of "good moment" needs work: noise floor
      below threshold, no recent rejected wake-bursts, page
      is article-shaped (long text, low interactivity), user
      has scrolled at reading pace not skimming-pace.
  
  Filed as separate ship from the rescue flow. Probably the
  *first* ship in the corpus area once we're ready, because
  it's lower-stakes (a friendly nudge vs an emergency
  diagnostic).

**Confirmed sequencing (v0.1.171, user direction):**

User locked in five ships, ordered. ALL voice corpus work
waits behind v0.1.172 image-lifecycle + modal-skip perf ship.
Then:

1. **Opportunity detection + basic git management.** The
   foundation. Two pieces in one ship:
     - Detection of "good moments": low noise floor +
       reading-pace scroll + long-form-content page +
       sustained-read-time threshold. The trigger for any
       opportunistic prompt.
     - Basic git primitives: auto-commit on voice-state
       change, snapshot/restore, list-snapshots. No bisect,
       no branches, no tags yet. Just "save," "list,"
       "revert." Without these primitives, no later ship
       has anything to hook into.

2. **Paragraph selection.** User taps on a paragraph (10-50
   words) and that becomes the read-aloud target. Default
   WPM (words-per-minute) configurable via tap-hold on a
   button somewhere — same affordance as the tap-hold
   button-expand from earlier in the session. The user
   chooses what to read; the system doesn't impose a script.

3. **Live words with dumb-DB highlighting.** Words highlight
   on screen as the user speaks them, matched against
   existing fingerprints. "Dumb DB" framing is explicit:
   don't try to be smart, just show the match — green for
   matched word, gray for not-yet, red for mismatch. The
   feedback loop is purely visual confirmation, no learning
   yet.

4. **Pacing them together (adaptive WPM).** System learns
   the user's actual reading pace from the live-highlight
   timing in step 3. The default WPM from step 2 becomes a
   seed; the adaptive WPM tracks the user. For a kid whose
   reading pace varies day-to-day, this matters. The
   highlight cadence follows what the user is actually doing.

5. **Advanced git actions and voice processing.** Bisect,
   named branches, tags, matrix evaluation, smarter
   fingerprint scoring, retroactive cleanup, all the L items
   we filed earlier. Built only after 1-4 are working in
   practice.

The 5-ship sequencing is the canonical plan for voice corpus.
Don't deviate without explicit user re-direction. The "voice
isn't working" rescue flow gets folded into step 1 (basic git
mgmt provides revert; step 5 makes it smart with bisect).
The cinematic onboarding + bottom-bar navigation + bluetooth
playback smoke test ideas from earlier are still filed but
not in this sequencing — they're orthogonal UI and infra
work, file but don't block the 5-ship arc.

**Scrolling patterns as cheap context source (v0.1.176 user
observation):** "huh yeah there are a lot of scrolling patterns
that could hint where someone is and what WPM they are at now
that I see you talking about it."

Scroll signals we already have access to that derive rich
context:

  - **Reading pace from velocity.** Slow steady = reading,
    fast continuous = skimming, erratic = searching. Feeds
    `Moment::is_reading_paced` (1b-β prereq).
  - **WPM from scroll-vs-visible-text.** Track how much
    text-content scrolls past per unit time; derive
    approximate WPM. Not exact (humans skip, re-read) but
    a useful tendency signal. Directly feeds ship 4's
    adaptive-WPM mechanism.
  - **Cognitive-load signals from pauses + reverse scrolls.**
    User stops, scrolls slightly back, re-reads. Signals
    "this section is hard or important." Could be folded
    into per-paragraph confidence scoring.
  - **Document-position semantics.** Top of page = scanning,
    middle = engaged, near-bottom = finishing or jump-to-
    conclusion. Each implies different downstream behavior
    (e.g. only prompt for corpus during "engaged middle"
    moments).
  - **Time-of-day modulating pace baseline.** Same person
    reads slower at 11pm than 11am. The adaptive-WPM (ship 4)
    should bucket per-time-of-day, not assume a single user
    baseline.

These compose naturally:
  - 1b-β's `is_reading_paced` plumbing should expose these
    derived signals, not just the boolean. The `Moment`
    struct could grow fields like `scroll_velocity_ema`,
    `wpm_estimate`, `recent_reverse_scrolls`.
  - Ship 4's adaptive WPM uses the same plumbing as a richer
    consumer — derives per-user-per-time-of-day WPM models.
  - A future "canvas memory" surface ("you read about this
    for 8 minutes last Tuesday") consumes the same data,
    persisted at coarser granularity.

Implementation hook for 1b-β: a `scroll_signals` module that
QML's `browseFlick.onContentYChanged` pushes velocity samples
into. Exposes `current_velocity()`, `is_reading_paced()`,
`estimated_wpm()` to the rest of the Rust side. The Moment
classifier reads from it; ship 4's WPM model writes to it via
learned corrections.

**Why this is filed, not built:** user said "keep working on
TODOs be back later" after asking three questions I asked
above. I shouldn't build before they confirm corpus text
default, no-corpus trigger behavior, and per-speaker vs global
scope. Picking defaults without confirmation risks shipping
something that doesn't fit their kid's actual use.

**Honest framing on scope:** even the "first ship" is M-sized
to L-sized. Auto-commit (S to M), corpus storage (S),
capture flow (M), matrix evaluation (M), opus encoding (S if
crate exists, M if it needs to be added). Total: probably 2-3
ships, not one. That's OK — it's the *feature area* the user
explicitly identified as mission-critical for their son. The
spend is justified.

### Self-recovery as a pattern (long arc, filed)

User insight from same conversation: "if we have a hard time
with it, normal people will too." The voice recalibration
corpus is the first instance of a generalizable pattern —
**anything the system updates over time needs a versioned undo
the user can invoke without expert knowledge.** Filed as the
meta-principle to consult when touching any adaptive subsystem:

- LLM cache: when a Claude response is wrong, can the user
  invalidate it for this query and retry? Today: no.
- Command index: when phonetic confusions get learned wrong,
  can the user roll back just those entries? Today: no, only
  via full wipe.
- Token stats: same.
- Decision cache: same.

Each subsystem currently has its own ad-hoc reset (mostly
"delete the bin file"). The pattern says: *snapshot before
mutating, expose a user-driven recovery path, never require
expert intervention.* Don't refactor all of them now. Apply
the pattern the *next* time we touch any one of them. File so
the next-time-we-touch reminder is here.

### Soft guidance for kids (long arc, filed)

User direction v0.1.172: "directories could be tied to users
and ask are you sure you want to go here... or some system
integration to help guide kids like you said with the text
guidance... with great power comes great responsibility but
kids need help managing that with guidance and never hard
limits that promote workarounds psychologically."

The framing is the most important part: **soft guidance,
never hard blocks.** Hard parental controls teach kids to
circumvent — VPNs, hidden devices, friend's accounts. The
system becomes adversarial. Soft prompts that inform without
blocking keep the system on the kid's side. The popup is a
moment to think, not a wall to climb.

**What it looks like in practice:**

- Identified kid profile is active (via voice fingerprint,
  settings, or both — uses the same "identified speaker"
  infrastructure as voice recalibration corpus).
- Kid navigates to a path/URL/setting tagged "usually for
  grown-ups" or "not in known-safe list."
- App shows a friendly, non-shaming popup: "this folder is
  usually for grown-up work — want to go in anyway?" with
  "yes, continue" and "no, go back."
- "yes" proceeds, optionally logs for parental review.
- "no" returns to previous context.
- No password, no block, no "ask a parent" gate.

**Design constraints (mission-critical for this to work):**

1. **No shaming.** Copy must never imply "you shouldn't be
   here." Always informative/confirmatory, never judgmental.
2. **No friction loop.** System learns "this kid visits this
   path often, stop asking." Soft guidance dissolves into
   background as kid demonstrates intent.
3. **Parent visibility, not surveillance.** Log enough for
   the parent-kid conversation, not everything. Visit counts
   yes, full keystroke logs no.
4. **The kid can opt out.** "Stop asking about this folder"
   option in the popup. Parent sees the opt-out logged, can
   talk about it offline.
5. **Composable with rescue flow.** "Want help finding your
   way back?" framing for kid who lands somewhere broken.

**Composes with:**
- Voice recalibration corpus (uses identified-speaker
  infrastructure)
- Onboarding/text-guidance content (same friendly voice)
- Self-recovery pattern (the popup is a kind of soft
  recovery affordance)
- Future profile system that distinguishes kid/adult/other

**Implementation depends on:**
- Per-profile config (which paths are "usually for grown-ups"
  for this kid) — needs a profile data store
- Identified-speaker detection working reliably enough to
  attribute navigation events correctly
- A parental review surface (probably a settings panel
  section, not in the kid's main UI)

**Filed, not built.** The voice recalibration corpus arc
(5 ships) needs to land first because it provides the
identified-speaker infrastructure this depends on. Once that
ships, kid-guidance is a natural follow-on.

## Pending — quality / smaller items

### Voice quality

- [ ] **S — RMS=1.1689 (>1.0) clipping anomaly investigation.**
  v0.1.135 log showed occasional `rms=1.1689` on rejected loud
  chunks. RMS of normalized f32 audio should physically be ≤1.0.
  Possible causes: i16-to-f32 conversion not clamping (denormals
  or saturated values producing > 1.0 after divide-by-32768);
  buffer aliasing where stale memory is interpreted as audio;
  cpal stream callback receiving partial frames. Investigation:
  add a clamp+log line in the F32 callback when value > 1.0;
  see if it's a specific device/sample-format combination. Cheap
  to add an assertion; root cause likely points at one of the
  defensive paths from v0.1.136 needing tightening.

- [ ] **S — "Me" allowed=false → true state-toggle glitch.** v0.1.135
  and v0.1.138 logs show occasional `[speakers] "Me" allowed=false`
  followed shortly by `[speakers] "Me" allowed=true` without a UI
  action that should have toggled. Smells like a state-management
  race during some flow — possibly the per-speaker audio routing
  reads the flag once, then a second path resets it. Trace which
  call site writes `allowed=false` during normal operation
  (search for `set_allowed`, look for any non-user-driven
  invocation). Fix the unwanted reset.

- [ ] **S — Whisper hallucination prefix stripping firing
  constantly.** Logs show repeated `Voice: stripping whisper
  hallucination prefix "and "` / `"so "` / `"you "`. The
  stripping mitigation works, but firing on near-silence means
  whisper is being asked to transcribe audio that's mostly noise.
  Either (a) whisper inference is being submitted for too-short
  bursts, or (b) the burst-end detection is too generous
  (capturing trailing silence that whisper confabulates against).
  Investigate burst threshold for wake-listen; tighten if so.
  Reduces whisper compute waste AND log noise.

- [ ] **M — Spectral voicing detector for noisy environments.** v0.1.130
  added ZCR-based voicing gate (drums/bass/cymbals get rejected at
  the easy end). True noise cancellation needs frequency-domain
  analysis: FFT of each 100ms chunk, compute energy in voice
  fundamental band (80-300 Hz) and harmonic bands (1-4 kHz),
  reject if energy is concentrated outside those bands. Use
  realfft or rustfft (already a transitive dep). Optionally:
  spectral subtraction — estimate noise spectrum during silence,
  subtract from speech-window spectrum to clean up the signal
  whisper sees. Target: subway noise, coffee shop chatter,
  background music without obvious vocals.

- [ ] **S — Tree-derived phonetic fingerprints.** When user registers
  a command, also derive fingerprints for known orthographic variants
  (e.g. "ls -lh" and "list files" treated as related). Mostly speculative;
  defer until user demand.
- [ ] **S — Sub-word phoneme decomposition.** Match partial fingerprints
  for words like "github" even when user voices half of it. Defer
  until phonetic matching shows clear cases where this helps.
- [ ] **S — Background AI corpus building.** During idle, transcribe
  recent terminal output and build a corpus for better Q&A grounding.
  Speculative.
- [ ] **S — Voice gdb stepping.** "Step over", "continue", "set
  breakpoint on line 47". Niche but cool for live debugging.

### Security

- [ ] **L — Secret Service encryption.** Encrypt persistent voice data
  (fingerprints, decision cache) using freedesktop secret service.
- [ ] **L — TPM-bound keys.** See `../TPM_keys_design.md`. Hardware-
  bound encryption for the most sensitive data.

### Recent-fact lookup

- [ ] **M — Recent-fact lookup.** See `../TODO_recent_fact_lookup.md`.
  Web search the latest news for time-sensitive queries.

### Async man-page fetch

- [ ] **S — Async man-page fetch.** Pre-fetch `man <cmd>` when a
  command is typed; cache for instant help. Particularly useful for
  missing-package detection (fetch man for the suggested command).

### Bugs awaiting repro

- [ ] **exit helix double-write.** Reported once; not reproduced.
  Needs more info.
- [ ] **Link hover broken (user report).** Verify v0.1.108 fix
  resolved it.

## Done recently (last few versions)

- ✅ v0.1.170 — Profiling infrastructure (WEB2QML_PROFILE=1 →
  JSON-per-3s window to ~/.config/web2qml/profiling.log with
  min/p25/p50/p75/max fps + dt_p, touch counts, event labels,
  current page URL). File-picker column-pan one-direction
  fix (replaced ScrollBar.position spring with explicit
  NumberAnimation on contentItem.contentX, OutCubic 240ms).
  Filed cafe-test voice diagnosis (pitch-hint unit bug,
  threshold drift, wake-word multi-phrase, enrollment-gating),
  markdown table+math limitations, Apple.com text-not-cached
  hypothesis (deferred to next measurement-driven ship),
  recalibration-corpus feature area, self-recovery pattern.
- ✅ v0.1.169 — RenderTier v1: viewport-gated opacity for scroll
  perf. Wrapper Column exposes _vpTop/_vpBot; browseFlick
  throttle-updates them every ~200px scrolled; top-level
  children of body get wrapped in opacity-gated Item that
  Qt's renderer paint-skips when opacity=0.
- ✅ v0.1.168 — Browser JS→Rust consolidation: backend.nav() qt_method
  added, `W` constant simplified to `parent.width`, `<details>` uses
  id-based binding via FilterStats.total_elements as uniqueness
  counter. Parse-time perf pass.
- ✅ v0.1.167 — Aho-Corasick marker dictionary (added aho-corasick),
  swap-protected anthropic_api_key as Arc<Mutex<Option<SecretBytes>>>,
  voice_intent_toast opt-out, qml_gen.rs perf pass.

*Note: v0.1.142 – v0.1.166 entries are in `changelog/0.1.1x.md`,
not duplicated here. The list below picks back up at v0.1.141
for older context.*

- ✅ v0.1.141 — Bug fixes: picker headers (defensive fallback +
  diagnostics), drag-to-restore (PointHandler not MouseArea),
  Ctrl+Shift+C/V (actual fix in onShortcutOverride), typing-key
  gate for compression
- ✅ v0.1.140 — Toolbar compression Option B sticky (compressible
  buttons shrink on typing, horizontal drag expands), arrow auto-
  repeat at hardware-keyboard cadence, Page Up / Page Down buttons
  appear in pager mode
- ✅ v0.1.139 — Ctrl+Shift+C / Ctrl+Shift+V no longer eaten by
  termKeyArea (selection clipboard works), 📁 button on touch bar
  to open file picker at cwd
- ✅ v0.1.138 — Build fix: stray derive captured Cadence
- ✅ v0.1.137 — Cadence monitoring foundation: token timestamps in
  whisper, per-word timing extraction (segment-level), per-speaker
  Cadence struct with EMA updates, tooltip exposure for debug
- ✅ v0.1.136 — Defensive audio device selection: candidate list +
  retry loop + supported-configs fallback + better failure logging
- ✅ v0.1.135 — Async voice_listen_dual (UI no longer blocked during
  whisper inference), file mobile whisper-choke single-speaker fix as M
- ✅ v0.1.134 — Themed selection color (uses th.ac), hold-press-to-clear
  gesture on selection
- ✅ v0.1.133 — Deep mute mode (3-state long-press cycle, slash
  overlay) + manual ✕ feedback into negatives DB
- ✅ v0.1.132 — Fingerprint polarity (positives + negatives) Phase A:
  FP05 schema, negative-suppression scoring, polarity-aware eviction
- ✅ v0.1.131 — Configurable preview cadence (atomics + env vars +
  Settings UI), muted-speaker background channel (italic + dim)
- ✅ v0.1.130 — Pager-mode smooth-scroll disable, selection
  reconcile on output signal, ZCR voicing gate in VAD
- ✅ v0.1.129 — Fixed QML load failure: function name collided with
  auto-generated property change signal
- ✅ v0.1.128 — Multi-user: chronological word seating across
  speakers + diff-based per-speaker replacement
- ✅ v0.1.127 — Folder identicons, picker style toggle, "+ another
  speaker" affordance
- ✅ v0.1.126 — Cursor position bug fixes (hide during scrollback,
  defensive clamp)
- ✅ v0.1.125 — Voice lexicon ("go up one directory", "open file
  picker"), macOS-style column file picker
- ✅ v0.1.124 — Wake RMS bug fix, wake-phrase guard, pool fallback,
  QML TypeError, selection-follows-content anchors
- ✅ v0.1.123 — Wake-word sensitivity tightened, candidate click
  guard, explicit-binary candidate surfacing
- ✅ v0.1.122 — Confirm-listen latency tightening (yes/no replies
  ~600-900ms faster)
- ✅ v0.1.121 — Wake-word inline command (`"{keyword}, open terminal"`
  now works as a single utterance)
- ✅ v0.1.120 — Per-speaker audio routing (pool-based finalization)
- ✅ v0.1.119 — Per-chunk Separator wired, register-self UI, hard
  whitelist gating
- ✅ v0.1.118 — docs/ folder
- ✅ v0.1.117 — Live multicolor view + speaker circles
- ✅ v0.1.116 — Worker pool + separation library + speaker whitelist
- ✅ v0.1.115 — Workflow voice triggers + French parser + summarizer
  timeout fix
- ✅ v0.1.114 — Spanish intent parser + workflow recorder foundations
- ✅ v0.1.113 — dust/fastfetch column alignment fix (HTML whitespace),
  QML startup warnings cleanup
- ✅ v0.1.112 — phonetic_confusions.toml parse spam fix, QML duplicate
  id fix
- ✅ v0.1.111 — Compile fixes for v0.1.110
- ✅ v0.1.110 — Natural language pathways + multilingual foundations
- ✅ v0.1.109 — Multi-style fingerprints with style ID

Full historical log: `../TODO_fingerprint_db.md`

## Triage notes

When picking what to work on next, considerations:

- **User-reported pain** > feature work. If a regression appears in
  a stderr log, fix that first.
- **Visible payoff** > backend infrastructure (when neither is
  blocking). UI feedback teaches the user the system works;
  invisible improvements don't.
- **Single-purpose vs. compounding.** Workflow recorder is single-
  purpose; speaker registry compounds (enables future features).
  Prefer compounding when comparable effort.
- **Defer if blocked on user feedback.** Anything marked "needs repro"
  or "needs spec" goes back to user for input before coding.

### v0.1.158 follow-ups

- [ ] **M — Voice button spacious-vs-compact mode + scrollbar
  notifier alternative.** User feedback v0.1.158: "the voice
  button does not shrink when the toolbar is dragged and it
  gets in the way". Two parts:

  Part 1 (S): tag the voice entry in `modRow.model` with
  `c:true` like other compressible entries. This is the
  simple width-only fix — voice button shrinks during typing.
  Risk: voice is a big tap target by design (so users who
  can't fine-control their device can hit it). Shrinking
  removes that. Need a way to opt out of compression for
  the voice button specifically.

  Part 2 (M): "spacious vs compact" setting. Spacious mode
  (default for accessibility-need users):
    - Voice button never shrinks
    - Voice button remains big tap target
    - Terminal scrollbar gets extra padding on the right
      side equal to the voice button's effective width so
      they don't overlap visually
    - Could collapse to a small scrollbar-side notifier
      (LED-style indicator + small tap target) when
      compressed
  Compact mode:
    - Voice button shrinks per part 1
    - Scrollbar uses normal padding
    - Visible always
  Default depends on accessibility preference; ideally
  detected from OS but most users will pick.

  Filed for design session before implementation. Real
  interaction model decision.

- [ ] **M — NLP coverage for command flag sets.** User
  feedback v0.1.158: voice commands like "show me colors"
  or "show me a table" should map to `ls --color=always`
  or `ls -L` or `column -t` etc. Currently only handful
  of intents are wired. Approach:
    1. Audit `tldr` cache (already shipped) for common
       command + flag combinations
    2. Extract canonical "want X" → "flags Y" mappings
    3. Surface as voice intent extensions:
       - "show me colors" → enable color output (cmd-aware)
       - "show me a table" → tabular output (cmd-aware)
       - "show me dates" → date columns (cmd-aware)
       - "show me sizes" → human-readable sizes
       - "ignore case" / "case insensitive"
       - "include hidden"
       - "follow symlinks"
       - "recursive"
    4. The "(cmd-aware)" piece is the hard part — needs
       to know what the user's CURRENT foreground command
       is, OR the last command run, to know which flag
       set applies. Re-use term_foreground_command logic
       from v0.1.140.
  Filed for its own ship — surface area is substantial.

- [x] ~~**M — WebView rendering performance for large pages.**~~
  Partially landed v0.1.159 — the per-link-MouseArea hover plumbing
  was the dominant cost on basic Qt pages. Stripped the
  `property bool _hover` + `hoverEnabled: true` + `onContains
  MouseChanged` plumbing from generated `<a>`, `<button>`,
  `<iframe>`, `<video>`, `<audio>` elements. Also gated the
  pull-to-refresh bindings to only evaluate during pull-down,
  not normal-down scroll.
  Trade-off: no hover-highlight on links on desktop. Mobile loses
  nothing (touch doesn't fire hover). The remaining filed items
  are still open:
    - Repeater vs ListView vs Loader for very long pages (would
      require modeling pages as lists, big refactor)
    - CSS fade-in / display-none class abstraction coverage
    - Off-screen lazy-evaluation gaps
    - `visible:false` vs `opacity:0` audit
  Refile as M if real measurement shows more wins to be had.

- [ ] **S — Missing packages pane: apk awareness (Alpine).**
  User feedback v0.1.158. The missing-package detector
  knows about apt and dnf and likely pacman; add apk for
  Alpine. Check current detection code (probably in
  src/main.rs `detect_missing_package` or similar):
    - Detect Alpine via /etc/os-release or `which apk`
    - Map missing-binary to apk-find equivalent
    - Suggest `apk add <pkg>` install hint
  ~30-line change in detect path + tests for resolution.

- [ ] **L — Backend code cleanup (continuous task).** User
  feedback v0.1.158: "look to make the code for this
  backend nicer as well". Not a one-time ship — should
  pick opportunities alongside feature work. Specific
  cleanup areas worth keeping in mind:
    - `Backend::new` is huge; extract initialization of
      individual subsystems into methods
    - PTY handling has retry/error paths threaded
      through; consider an error-recovery wrapper
    - Speaker/voice state has accumulated booleans
      (mode + active + allowed + drift + lockout);
      consider a state enum
    - Save/load_settings is a flat tuple; gross
    - String-based protocol sentinels (\x01FAVORITES\x01)
      could be enum + dispatcher
  Each item is its own S; do them when in the area.

- [ ] **S — Identify abstraction opportunities (continuous
  task).** User feedback v0.1.158: "keep working on looking
  for places to make meaningful abstractions and functional
  blocks". Patterns to watch for:
    - QML row delegates with similar structure (file picker,
      block list, etc.) — pull into shared Component
    - JS array → ListView delegates need `modelData` consistent
      with the v0.1.158 fix
    - Backend C++/Rust bridge for property bindings —
      formalize the null-guard pattern that the lint catches
    - Toast severity + clipboard ops form a "user-feedback
      action" pattern — could be extracted
  Note opportunities; ship them when they're worth the
  refactor cost.

### Test-suite reconciliation (v0.1.875–876)

Swept 11 pre-existing `cargo test` failures (the ship-gate runs `cargo
check`, so failing assertions never blocked it). 8 were real bugs or
stale tests, now fixed:

- [x] recipes::every_main_rs_url_has_a_recipe — mail-bridge routes
  (`web2qml://mail{,/login,/settings}`) + the `man/{}` format template
  were unregistered; added to the recipes-test ALLOWLIST.
- [x] voice_intent::no_orphaned_recipes — the `tile-ssh` recipe used a
  made-up `kind`; it produces `Intent::TermPane` → relabeled to the real
  wire kind `term_ws`, and enumerated `TermPane` in `all_intents()`
  (it had been omitted, so `term_ws` wasn't a known kind).
- [x] voice_intent::open_connections_view_parser (partial) — "show me my
  last session" broke when the v0.1.708 `"show me "→"show "` reduction
  rewrote it; added the normalized form to the WalkLastSession arm.
- [x] manpages italic/bold/underscore_{italic,bold} — paragraphs FLATTEN
  inline markup by design (selection-wrapper rationale); repointed the
  tests at `inline_to_richtext` (used by headings/lists/blockquotes),
  the real emphasis→`<i>`/`<b>` contract.
- [x] manpages::to_qml_paragraph_wraps_for_selection — used a 4-word
  input below the intentional 10–50-word selection-wrap bound; widened.
- [x] manpages::welcome_doc_renders_clean — welcome.md's H1 was
  shortened to "Welcome"; updated the assertion.
- [x] manpages::wikilink_to_qml_links_with_onLinkActivated — asserted an
  unescaped `href="..."` but the href sits in a QML `text:"..."` literal
  (quotes backslash-escaped); assert on the URL substring instead.

Three remain — each a genuine PRODUCT DECISION where a later
normalization/feature collides with an earlier test's intent, and a
blind code change risks regressing an intended feature + other corpus
cases. Need a call on the intended behavior:

- [ ] **voice_intent::open_connections_view_parser ("what did i just
  do").** `strip_fillers` removes "just" → "what did i do", which the
  SummarizeSession arm claims before recall. But "just" carries the
  "immediate last action" sense that distinguishes recall-last-command
  from summarize-session. Decide: should "what did i just do" →
  RecallCommands (and stop stripping "just" here) or SummarizeSession
  (relax the test)?
- [ ] **voice_intent::narration_open_tail_not_searched ("open rust async
  docs").** The v0.1.380 sub-path feature maps "rust" + "async docs" →
  rust-lang.org/async/docs (like "github torvalds linux" →
  github.com/torvalds/linux). The test wants it to web-search. Decide:
  should "open <known-site> <query words>" navigate a sub-path or search
  (and if search, how to keep github-style sub-paths working)?
- [ ] **combo_detect::proactive_silent_above_threshold.** A 2-command
  workflow repeated 4× also produces the wrap-around 2-gram exactly 3×,
  which trips the ==threshold one-shot proactive suggestion. Decide:
  should the N-gram windower ignore wrap-arounds, or should the
  proactive suggestion require the dominant (max-occurrence) sequence?

## Voice: sub-path nav vs web search (pending decision) — v0.1.900
`resolve_known_site` stitches spoken words after a known-site key into a URL
path: "open github torvalds linux" → github.com/torvalds/linux (intended). The
same logic turns "open rust async docs" → rust-lang.org/async/docs instead of a
web search for the phrase. For platform sites (github: user/repo) the sub-path
is meaningful; for content sites (rust-lang.org, wikipedia, apple) appending
arbitrary spoken words as a path is usually a wrong URL and a search would serve
the user better. Decision needed: restrict sub-path stitching to an allowlist of
platform sites (and route the rest to SearchWeb), or keep the current broad
behavior. Test `narration_open_tail_not_searched` currently asserts only the
weaker invariant (navigate OR search) pending this call.
