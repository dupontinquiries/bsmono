# Paperbound — AI Director ("Living Page")

The **Living Page** game mode hands the battlefield to an AI game-master that
invents cards, swaps maps and throws twists at the players in real time — you
have to adapt on the fly.

It works two ways:

- **Procedural (default, offline).** No setup, no network. A weighted heuristic
  director generates balanced cards and maps from templates. This is what runs
  if you don't configure an LLM.
- **LLM-powered (optional).** Point it at any OpenAI-compatible chat endpoint —
  OpenAI, a local **Ollama** / **LM Studio** / **llama.cpp** / **vLLM** server,
  Groq, Together, or Anthropic — and the director *asks the model* what to do
  next given a JSON snapshot of the battle.

Either way, **every card and map the model produces is validated and clamped**
by `ai_director.sanitize_card()` / `sanitize_map()` before it touches the game,
so a malformed or adversarial model response can never break the simulation.

## Enabling an LLM

Set environment variables before launching:

```bash
# OpenAI
export PAPERBOUND_LLM_KEY="sk-..."
export PAPERBOUND_LLM_MODEL="gpt-4o-mini"          # optional
python game.py

# Local Ollama (no key needed — localhost auto-enables)
export PAPERBOUND_LLM_BASE="http://localhost:11434/v1"
export PAPERBOUND_LLM_MODEL="llama3.1"
python game.py

# LM Studio / llama.cpp / vLLM (OpenAI-compatible server)
export PAPERBOUND_LLM_BASE="http://localhost:1234/v1"
export PAPERBOUND_LLM_MODEL="your-model"
python game.py

# Anthropic
export PAPERBOUND_LLM_PROVIDER="anthropic"
export PAPERBOUND_LLM_KEY="sk-ant-..."
export PAPERBOUND_LLM_MODEL="claude-3-5-haiku-latest"
python game.py
```

Recognised variables (falls back to the standard `OPENAI_*` names too):

| Variable                     | Meaning                                   | Default                     |
|------------------------------|-------------------------------------------|-----------------------------|
| `PAPERBOUND_LLM_BASE`        | Chat endpoint base URL                    | `https://api.openai.com/v1` |
| `PAPERBOUND_LLM_KEY`         | API key (not needed for localhost)        | —                           |
| `PAPERBOUND_LLM_MODEL`       | Model name                                | `gpt-4o-mini`               |
| `PAPERBOUND_LLM_PROVIDER`    | `openai` \| `anthropic`                   | `openai`                    |
| `PAPERBOUND_DIRECTOR_PERIOD` | Seconds between director actions          | `14`                        |

The LLM is only enabled when a key is present, or when the base URL points at
`localhost`/`127.0.0.1` (a local model server). Otherwise it stays procedural.

## Safety & performance

- **Non-blocking.** All network calls and DSP happen on background threads and
  hand results to the game through a thread-safe queue; the render loop never
  stalls on I/O.
- **Validated JSON.** Stats are clamped to safe envelopes, abilities are checked
  against the known set, colours are range-checked, buildings are forced to
  `spd 0`, swarms can't be tanks, etc.
- **Graceful fallback.** Any LLM failure (no network, bad JSON, timeout) silently
  falls back to the procedural generator, so the mode always keeps playing.

## Card / map JSON contract

A generated **card**:

```json
{
  "id": "emberfox", "name": "Ember Fox", "cost": 3,
  "hp": 300, "dmg": 66, "spd": 96, "rng": 22, "size": 19,
  "flying": false, "building": false, "riverhop": false, "count": 1,
  "paper": [210, 110, 50], "ability": "molten",
  "rarity": "rare", "set": "living", "blurb": "Leaves a burning trail."
}
```

A generated **map** carries a palette (`page`, `desk`, `cover`, `river`,
`ribbon`), a `weather` and `hazard` tag from the engine's known set, `candles`,
and flavor. See `ai_director.py` for the exact fields and bounds.

## Extending

- Add new engine abilities to `ABILITIES` in `ai_director.py` (and implement the
  behavior in `game.py`) so the smiths can use them.
- Tune archetypes/balance in `procedural_card()`.
- The `Director.decide()` heuristic weights how often each twist happens; adjust
  there, or let the LLM drive it entirely.

## Arch-Scribe bosses (`ai_boss.py`)

The **Arch-Scribe Duel** on the main menu launches an LLM-driven boss that reads
your board and *answers your style*. Each boss is a `BossPersona` with a distinct
voice, tactical bias, minion palette and signature abilities:

| Persona          | Style                                                        |
|------------------|--------------------------------------------------------------|
| The Inkwright    | Cold eraser — blots your crowded lanes, freezes your best.   |
| The Mirrorbinder | Copies your strongest card back at you; hard-counters comps. |
| The Swarm-Mother | Drowns the page in cheap numbers, floods the flanks.         |
| The Chrono-Scribe| Freezes your advances, hastens its own, punishes over-commit.|

Like the Director, a boss has two brains:

- **Heuristic (offline):** `read_threat()` classifies your board (`swarm`, `tank`,
  `air`, `turtle`, densest lane, biggest unit) and ~65% of the time the boss
  answers it directly — zoning a swarm, freezing/mirroring a tank, contesting air,
  cracking a turtle with artillery — otherwise it leans on its persona's bias.
- **LLM:** the persona's `voice` is injected as the system prompt and the boss is
  handed the battle JSON (plus a `threat` summary and your recent plays). It
  replies with ONE action from a fixed safe vocabulary
  (`summon / counter / zone / mirror / buff / taunt`). Every response — including
  any custom minion it "writes into being" — is validated through
  `sanitize_action()` / `ai_director.sanitize_card()`, so the boss can never emit
  anything the engine can't safely apply.

The boss speaks in-voice on screen as it acts ("*The Inkwright: 'Your strongest
line — unwritten.'*"), acts faster as its king weakens, and — because it's
reacting to *you* — rewards changing your plan rather than repeating one opener.

Env: `PAPERBOUND_BOSS_PERIOD` (seconds between boss actions, default 9). The LLM
config is shared with the Director (`PAPERBOUND_LLM_*`).

## Origami field models (`origami.py`)

Each creature on the field is folded from a small vocabulary of **known-good part
primitives** — `slab, panel, wedge, wing, spine, leg, crest, tusk, tail, shell,
orb`. A card's field model is a JSON **recipe**: an ordered list of parts, each a
small object in body-local coordinates:

```json
"model": [
  {"part": "slab",  "at": [0, 0],      "size": 1.0, "h": 0.55},
  {"part": "wedge", "at": [0.85, -0.35], "size": 0.8, "ang": -0.15, "tone": 1.1},
  {"part": "tusk",  "at": [0.95, -0.05], "size": 0.7, "ang": -0.5},
  {"part": "tail",  "at": [-0.95, -0.1], "size": 0.65, "ang": 0.4, "tone": 0.9}
]
```

Because a model is plain data it is **serializable, loggable, diffable, and
AI-authorable**. Resolution priority per card:

1. `card["model"]` — an explicit recipe (validated via `origami.fold_sanitize`),
2. the **family default** recipe (procedural, deterministic per key),
3. `card["model_extra"]` — optional appended accent parts.

**Safety:** every recipe passes through `fold_sanitize()`, which drops unknown
part names and clamps all numbers to safe envelopes — so a model invented by the
LLM (the Director validates `model` in `sanitize_card`) can only ever reference
known-good pieces and can never break the renderer. Models are **cosmetic only**:
sim hitboxes are unchanged, so net determinism is unaffected.

**Flyer separation:** units with `flying: true` lift off the page (altitude with a
gentle hover bob), their drop-shadow **detaches and shrinks** on the ground with a
visible gap, and their wings flap faster — so airborne pieces read as flying at a
glance. Grounded pieces keep a contact shadow.

To give a generated card a unique look, have the model emit a `model` array; to
tweak an existing one, edit its `model` in `cards.json`. Family defaults live in
`origami._recipe_for_family()`.

### AI-authored models

Generated creatures now arrive with their own origami fold, not a generic shape:

- **Procedural** — `ai_director.procedural_model(archetype, rng)` builds a fitting
  recipe per archetype (flyers get wings + beak + tail, tanks get shell + spines,
  brutes get legs + head + tusks, casters get a hooded head + staff-orb, etc.).
  Every card `procedural_card()` makes includes a valid `model`.
- **LLM** — the card system prompt teaches the full fold vocabulary and asks the
  model to compose a fitting silhouette. Whatever it returns is validated by
  `origami.fold_sanitize()` inside `sanitize_card`, so only known-good parts ever
  reach the field.
- **Boss summons** — `BossMind._minion()` tags each summon with a matching
  archetype model, so an Arch-Scribe's creatures look distinct and in-persona.

Because models are cosmetic and always sanitized, AI authorship never affects
balance or net determinism.

## Boss lookahead simulator (`boss_sim.py`)

A lightweight forward model the AI boss uses to **evaluate a card before it plays
it**. The real battle sim is deterministic but too heavy to fork per candidate,
so this is a compact abstraction of the same rules (lanes, movement, range,
dmg/hp attrition, defended kings) that runs hundreds of rollouts instantly. It's
**read-only/advisory** — it reads a `_boss_state` snapshot and returns numbers,
never touching the live game, so it can't affect determinism.

The boss uses it three ways:

- **Budget** — `evaluate_summon` / `should_play` score a candidate and divide by
  its ink cost, so the boss can skip low-value plays and spend where it counts.
- **Balance** — `balance_probe` classifies a card as `weak` / `fair` /
  `oppressive` across canned scenarios (swarm, tank, air, turtle, poke), and
  `autobalance` nudges hp/dmg toward a "strong but fair" survival band (~55%
  hp remaining after a good trade). Every heuristic summon is autobalanced, so
  boss creatures hit hard without being un-fun.
- **Timing / lane** — `best_deployment` tries every lane and returns the one that
  best answers the current board, so the boss actually intercepts the player's
  busy lane instead of dropping minions blindly. The chosen lane is passed to
  `apply_boss_action`, which places the summon there.

LLM-driven summons flow through the same evaluator (`_sanitize_action`): the
model's lane is honoured if given, else the sim picks the best, and the eval is
attached — so even an LLM boss's plays are grounded in a forward estimate. The
scoring rewards damage dealt, kills, king pressure, and *contesting the busy
lane*, and lightly penalises losses — tuned so the boss answers the board rather
than dodging every fight.
