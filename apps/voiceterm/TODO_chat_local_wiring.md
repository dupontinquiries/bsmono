# Chat ↔ local-model wiring (started v0.1.551)

## Done (v0.1.551) — backend only, NOT yet runtime-verified
`chat_send` (main.rs ~7408) now prefers the local llama model when one
is already loaded, falling back to cloud (Claude/OpenAI) otherwise.

- Reuses the PROVEN local-model detection from `llm_query` (try_lock
  guard, `is_available()`, never lazy-loads on the UI thread).
- New testable helpers in `chat.rs`:
  - `push_user_and_build_prompt(msg) -> Option<String>` — appends the
    user turn + returns a flat role-tagged prompt.
  - `build_flat_prompt(&[ChatMessage]) -> String` — PURE, 4 unit tests
    (verified standalone in sandbox: all pass).
  - `push_assistant_response(resp)` — appends the assistant turn.
- Routes through `provider_query_with_local(LlamaCpp, "", prompt, ...)`
  — same call shape as llm_query (verified arg-by-arg).
- Redaction: applies `context_strip_tiered` before the local call.
- On local error: surfaces the error AS the assistant message and does
  NOT silently fall back to cloud (avoids leaking a local-redacted
  prompt to the cloud).

### Breadcrumb logs to watch when testing (all via clog!, reach stderr)
- `[llm] chat_send: routing to LOCAL model`
- `[llm] chat_send: local reply (N chars)`
- `[llm] chat_send: local query failed: <e>`
- `[llm] chat_send: routing to CLOUD provider=<p>`
The FIRST of these on a chat send tells you which path ran.

## NOT done — needs the live QML surface (do when testable)
1. **(DONE v0.1.553) Chat UI in QML.** ChatPanel.qml — full-window Item
   overlay (mirrors SetupWizard's proven pattern, NOT Popup-rooted).
   Scrim + centered card, message ListView (role bubbles: user = right/
   blue, assistant = left/gray), input box → chat_send_async, Timer
   polling chat_poll_result while chat_busy, "thinking…" indicator +
   New/close buttons. Instantiated in main.qml with backend:
   root.ctxBackend (NOT backend: backend — shadow trap). Opens via:
   settings "💬 Chat with local model" button, OR the
   web2qml://chat-open / web2qml://chat-panel URL route (new
   chat_panel_requested signal → onChat_panel_requested Connections).
   Breadcrumbs: [chat-qml] ChatPanel instantiated / open() called /
   reload: N message(s) / send: dispatching (N chars) / signal received.

   WATCH WHEN TESTING (this is un-runnable live QML — most likely spots):
   - At startup expect "[chat-qml] ChatPanel instantiated; backend=
     present". If absent → component didn't load (check for QML errors).
   - Open via settings → "[chat-qml] settings 'Chat' clicked" then
     "open() called" then "reload: N message(s)".
   - The \x1F split in reload(): QML string.split("\x1F") — verify the
     escape works (JS \x1F is valid). If bubbles show the whole raw line
     incl. the M prefix, the split failed → use String.fromCharCode(31).
   - ListView model = chatPanel.messages (a JS array). If the list
     doesn't refresh after reload(), the array-reassignment may not be
     triggering the binding — wrap in a property change or use a
     ListModel instead.
   - delegate uses modelData.role/.content — confirm those land.

2. **(DONE v0.1.552) Async chat.** [unchanged — see above]
3. **(DONE v0.1.552) Chat token budget.** max_tokens 1024 for chat.

4. **Chat template (quality refinement — needs per-model testing).**
   build_flat_prompt uses plain "User:/Assistant:" — model-AGNOSTIC and
   works for all instruct models, but suboptimal. llama_backend::generate
   takes a RAW prompt (no template applied). Proper format is per-family:
   Qwen2.5 = ChatML `<|im_start|>user\n…<|im_end|>\n<|im_start|>assistant\n`;
   Llama-3.2 = `<|start_header_id|>user<|end_header_id|>…`. Hardcoding one
   BREAKS the other, so this needs: (a) detect model family (the GGUF
   metadata / filename has it — "Qwen2.5" vs "Llama-3.2"), (b) pick the
   template, (c) TEST reply quality + clean stopping per model. Deferred
   because a wrong template makes replies WORSE than the plain format,
   and I can't observe quality without running it. Plain format is the
   safe floor until then.

## Verification status
Static: brace-clean, qt_struct 436 clean, qml_lint/backend_check clean,
build_flat_prompt logic verified standalone (4/4). NOT compiled in the
full crate (qmetaobject needs system Qt — sandbox can't). NOT run.
