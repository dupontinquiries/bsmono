# logscope UI/UX analysis

Grounded in the source (I can't run the GUI here, so this is read from `app.rs` + the tab
modules, not from using it). Structure: what it is → what works → friction + concrete proposals →
**questions where your intent matters more than my guess**. The questions are the point — answer
those and we can turn this into a real plan.

## What logscope's UX actually is (from the code)
- **A carved-stone tab shell** with 10 flat tabs — Sources, Peek, Tokens, Blocks, Rotate, Cargo,
  **Chat**, Code, Modify, Bloc — rendered as amber-inscribed slabs with a light-sweep animation.
  The bar already uses `horizontal_wrapped`, so it wraps on narrow widths (partial responsiveness
  already there).
- **A dense header**: title, Glass toggle, model picker, theme + variant combos, models-folder +
  native-dialog checkbox.
- **Two very different halves.** By line count: `chat.rs` is **4592 lines**; the other nine tabs
  total **~790**. The log-analysis tabs (Sources/Peek/…) are lightweight and utilitarian; Chat is
  a rich "mind-palace" — a 3D scene, a refractive "pearl", branching conversations, a `:`-command
  palette (`:a`, `:fa`, `:ans`), voice dictation, file generation with version history. The app
  even lands on Chat ("land on the mind-palace").

## What works well
- **Strong, distinctive identity** — carved stone + amber + glass + pearl is not a templated look.
- **The log path is clean**: Sources (load/paste/filter with regex+invert) → Peek (viewer with
  dedup `4×` counts + severity coloring). That original tool is tight and functional.
- **Chat is genuinely powerful** — branching, threading, context, voice, code-gen with a check
  step. The command palette is fast for power users.

## Friction + concrete proposals (tied to the kit you now have)
1. **The log→code workflow isn't built.** Peek shows log lines; Chat has `set_input()`; **nothing
   connects them.** So "jump from a log into a code solution" — the thing you couldn't judge —
   *doesn't exist yet as a path*; you've been doing it by hand (copy from Peek, switch to Chat,
   paste). **Proposal:** a context action on a Peek/Sources line (or selection) — "Explain this"
   / "Fix this" — that stages the line(s) into the chat composer (`set_input`) and switches to
   Chat. Small, concrete, and it turns the workflow into something you can actually *feel* and
   then keep or cut. This is the single highest-leverage UX change.
2. **10 flat peer tabs is a lot to scan** (worse on phone, where they wrap to 3 rows). They aren't
   really peers — there's a natural grouping: *Logs* (Sources/Peek/Tokens/Blocks/Rotate), *AI/Code*
   (Chat/Code/Modify), *Build* (Cargo/Bloc). **Proposal:** group the tab bar into 2–3 labeled
   sections, or a primary Chat + a "workbench" cluster. Pairs with `uikit_egui::nav` for phone.
3. **Chat control density.** Many controls are inline: threading, context, pearl + optical
   variant, fetch-deps, auto-continue, per-message thinking/edit/history, KV knobs. Some are
   everyday (send, mic, clear); some are backend-testing dials that "never found their footing".
   **Proposal:** split everyday controls from power/dev toggles (overflow menu on desktop,
   `controls_overflow` on phone via `chat_layout`). *Which is which needs you — see questions.*
4. **Header does too much.** Model/theme/variant/folder all live in the top chrome. **Proposal:**
   move theme/variant/folder into the new `bsettings` (they're app-wide prefs anyway) and keep
   only model + Glass in the header. Unifies with the fleet's settings story.
5. **Two design languages.** The workbench tabs and the palace don't share a visual system.
   **Proposal (conditional on your intent):** either lean in (workbench = plain, palace = rich, on
   purpose) or unify tokens via `uikit-egui` theme so the log tabs inherit the carved/amber look.

## Mobile / physical-UI (mechanics already staged)
`chat_layout` + `tab_layout` (this session, tested) decide the adaptive layouts; the render wiring
is the on-device step. The scene-as-backdrop + bottom-docked composer is the phone chat shape;
master/detail drill-in covers Sources/Peek/etc. The tab grouping (#2) matters most here.

## Questions for you (this is where I need you, not guesswork)
1. **Log→code bridge (#1):** is "explain/fix this log line → Chat" the intended workflow? Should
   it stage *the line*, *the line + surrounding context*, or *the whole filtered view*? One-shot
   ("fix this") or conversational?
2. **Chat controls (#3):** which are everyday vs dev/testing? My guess at everyday = send, mic,
   clear, threading; dev = pearl variant, fetch-deps, auto-continue, KV/context knobs. Right?
3. **Tab grouping (#2):** are the 10 tabs peers, or is Chat the home and the rest a "workbench"?
   Is the Logs/AI-Code/Build grouping the right carve, or do you think of them differently?
4. **Design languages (#5):** is the workbench-vs-palace split intentional, or would you like the
   log tabs to inherit the mind-palace look?
5. **The pearl + 3D scene:** central to the experience, or ambient? (Decides whether phone keeps
   it as an interactive element or demotes it to a backdrop.)
6. **What annoys *you* most** when you use it? Your lived friction beats my code-reading.

## What I could build next without more input (verifiable-ish)
- The log→code bridge action (small; `set_input` + tab switch already exist) — on-device wiring.
- Fold theme/variant/folder into `bsettings` (kit-integrated; partly headless-testable).
- The tab-grouping model as a pure, tested helper (like the layout cores).

---

## Direction (decided) + what's built
Your call: **the mind-palace is Chat-only** — it's the LLM-assisted planning workflow where a
spatial memory earns its keep. Every other tab is a pre-patterned action on a file type, so it
gets **speed, near-invisible chrome, and rich touch navigation between related functions** — the
snapclone/voiceterm feel. Branching elsewhere is intriguing but stays a **toggle that's unwired**
for now.

**Built this session (pure + tested, lean build): `workbench` model.**
- `uses_mind_palace(tab)` → true only for Chat; the render code skips the whole 3D scene/pearl
  layer on every other tab.
- Tabs group into workbenches — **Logs** (Sources/Peek/Tokens/Blocks/Rotate), **Code**
  (Code/Modify), **Build** (Cargo/Bloc) — with an ordered swipe sequence.
- `swipe(tab, dir)` = snapclone swipe-paging between *related* functions (no wrap, so you can't
  swipe out of a workbench by accident); `gesture_nav(tab, gesture)` maps touchkit SwipeLeft/Right
  to next/prev (voiceterm quick-access), leaves pinch to the tab for content zoom, and is inert on
  Chat (the scene owns drags).
- `BranchToggle` — flips, no effect yet (the deliberately-unwired affordance).

**On-device wiring (next, where the gui compiles):** on non-chat tabs, poll
`uikit_egui::touch::TouchSource` → `workbench::gesture_nav` → switch tab; gate the scene render on
`uses_mind_palace`; render the workbench label + swipe affordance; show the (inert) branch toggle.
The Chat tab keeps everything it has.

**Still open (from the questions above):** the log→code bridge shape (Q1), which Chat controls are
everyday vs dev (Q2), and your lived friction (Q6).

---

## logscope is three tools (the real mental model)
Not a flat tab pile — three distinct tools, two surgical + one spatial:
1. **Diagnose (surgical):** log rotation + quick system diagnosis. Meant to get you *started*, not
   to hand a full solution. Tabs: Sources, Peek, Rotate, Tokens. → fast, gesture-navigated UI.
2. **Chat (palace):** large development, fan-out exploration, research — the LLM-assisted thinking
   space. The *only* tool with a physical/spatial layer (it's where you think; e.g. the same
   engine built a "mind-palace of the body" in another session). Tabs: Chat.
3. **Forge (surgical, unfinished):** an LLM + touch + "super-keyboard" — functional block analysis,
   like coding in Scratch but for big-boy programs. Replaces the keyboard with block manipulation
   (AST blocs, block actions, cargo diagnostics). Tabs: Code, Bloc, Modify, Cargo, Blocks. →
   surgical, so it keeps the quick-access UI, **not** the palace.

The `workbench` model now encodes this: `Workbench::{Diagnose, Palace, Forge}`, `is_surgical(tab)`
(true for Diagnose + Forge), and `uses_mind_palace(tab)` (Chat only). Swipe order follows each
tool's natural flow (Diagnose: load→inspect→rotate→tokens; Forge: view→bloc→modify→check→palette).
Open: confirm Tokens (placed with Diagnose) and Blocks (placed with Forge) once they're built out.


---

## North star: the survival tool (self-sufficiency, not austerity)
*(Corrected from an earlier over-reach.)* "Survival tool" = a desert camper **on their own** with
broken hardware and no cloud — the property that matters is **autonomy**, not running on the fewest
resources. Two consequences:
- **Don't ration RAM or power at the app layer.** The kernel already does memory-pressure handling
  and energy-aware scheduling, and does them better; an app-level "frugal/survival mode" that drops
  features to save power is futile duplication. (The earlier `field_tier` ladder was that mistake;
  it's been replaced.) The one honest hardware question — can a model physically *fit* in available
  RAM — is `model_selector`'s job, and it already does it.
- **Optimize *utilization*, not scarcity.** On a computer, doing *more* is often the efficient
  path: a warm KV cache beats re-prefilling every turn, a prefix tree beats recomputation, spending
  cycles to save cycles is the whole point. The real levers are **memory utilization, cache reuse,
  and algorithmic efficiency** — all app-layer, all worth the effort. logscope already leans this
  way (the KV prefix-tree behind `--kv-tree`, model_selector's warm handoff); that's the direction
  to push, not artificial limits.

**Built: `autonomy`** (2 tests) — the one thing worth asserting in code: the *self-sufficiency
invariant*. Every core capability (Diagnose / Chat-palace / Forge) runs with **no external
services** — Diagnose is strictly `LocalOnly` (works even model-free), the palace/Forge
`PrefersLocal` (remote is an optional step-up, never a requirement). The test guards against a
future cloud dependency sneaking into a core path — a regression against the whole premise.

**Efficiency direction (the actual work):** hot-path allocation + cache behaviour in the pure cores
(like the audiomix no-alloc pass), and leaning into the KV prefix-tree reuse. Not power/memory
gating — the kernel owns that.
