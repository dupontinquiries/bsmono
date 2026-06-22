# TEST-ON-RETURN checklist (v0.1.550 → 0.1.554)

Five versions of live-surface changes shipped WITHOUT a run, since the
last user-verified build (v0.1.549). This walks them in DEPENDENCY ORDER
so a failure isolates to one layer instead of debugging all five at once.

Build: `cargo r --release --features "voice claude llamacpp"`
Watch stderr (the breadcrumbs below all go through clog!/log_message,
which DO reach stderr — console.log does NOT, that's why we never use it).

## Layer 0 — app still launches (regression gate)
- [ ] App starts, terminal + web view still work (nothing below broke
      the base). If the app won't start or QML won't load, check the
      "Loading QML from:" line + any QML error — likely ChatPanel.qml
      (new in 553) failing to parse. qml_lint passed, but runtime load
      is the real test.
- [ ] At startup: `[chat-qml] ChatPanel instantiated; backend=present`
      MUST appear. Absent → ChatPanel didn't load (the SetupWizard-style
      failure mode). Present → the new component is live.

## Layer 1 — active-model registry (v0.1.554, foundation)
Hard to observe directly; verified indirectly via chat working. No
dedicated log. If chat-local works (Layer 3), the registry publish is
fine.

## Layer 2 — chat backend, sync + async (v0.1.551 / 0.1.552)
Open chat (Layer 3) and send one message. Expect ONE of:
- [ ] `[llm] chat_send_async: spawning local worker` then
      `[llm] chat_send_async: worker done (N chars)` — local path works.
- [ ] `[llm] chat_send_async: no local model, cloud path` — fell to
      cloud (only if no model active; you have Qwen, so this would be
      unexpected → registry/path bug).
- [ ] Reply text is coherent (not truncated mid-word → bump max_tokens
      above 1024; not garbled → prompt format issue in build_flat_prompt).

## Layer 3 — chat UI (v0.1.553, highest risk)
- [ ] Settings → "💬 Chat with local model" → `[chat-qml] settings
      'Chat' clicked` then `open() called` then `reload: N message(s)`.
- [ ] Panel appears centered with scrim (like the wizard). If it logs
      open() but nothing shows → same class as the wizard Popup bug;
      but ChatPanel is Item-rooted so this should be fine.
- [ ] Type + Send → your message appears as a right/blue bubble
      immediately (sync user-turn append + reload).
- [ ] "thinking…" shows while busy, then the reply appears as a
      left/gray bubble. If the reply NEVER appears but the worker logged
      "done": the poll path or the ListView array-refresh is the
      suspect (see TODO_chat_local_wiring.md watch-items).
- [ ] If bubbles show raw `M`-prefixed text with weird chars → the
      String.fromCharCode(31) split failed (shouldn't — it's hardened).

## Layer 4 — download 416 fix (v0.1.550, isolated)
- [ ] Re-trigger a download of an ALREADY-complete model → should NOT
      log `status code 416` as an error; should complete silently.

## If something breaks
The breadcrumbs localize it to a layer. Paste the stderr around the
first missing/wrong log line. Per the arc's lesson: the FIRST absent
expected log is the fault site — don't debug downstream of it.

## Known non-bugs (don't chase)
- Async CLOUD fallback is synchronous (no chat_busy spinner) — by
  design, only when no local model.
- log_summary still churns Ollama retries if Ollama absent — the fix is
  DEFERRED (TODO_active_model_registry.md), not regressed.
