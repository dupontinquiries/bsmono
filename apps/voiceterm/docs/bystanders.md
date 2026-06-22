# Bystanders

Design for the voice-vote / audio-attribution surface — a visual
primitive that makes every voice in the room its own thing instead
of folding everything the microphone catches into a single "the
user said this" channel.

Status: **SHELVED DESIGN**, captured for reference. The principles
and the visual model are settled enough to build from if/when the
work is prioritized, but it is **not on the active roadmap**. The
priority right now is power-user table stakes (see
[power-features.md](power-features.md)) — the bystanders work
addresses a real problem but the problem is not what's currently
blocking dogfooding.

This doc preserves the design so the conversation doesn't have to
happen again from scratch. The visible-attribution insight, the
dot-stack visual model, and the "each voice stays its own thing"
framing are the load-bearing ideas worth carrying forward; the
specific shipping plan is provisional and can be re-thought when
the work is picked up.

For the audio-source / speaker-identification infrastructure this
would build on, see [voice-pipeline.md](voice-pipeline.md) and
[multi-user.md](multi-user.md).


## Why this exists

Web2qml's voice subsystem today treats whatever the microphone
captures as a single intentional stream — "the user said this." In
practice, the microphone captures the user, their pair-programming
partner, the radio in the background, someone walking past saying
something to a colleague, and (in a hostile case) someone trying
to inject commands via voice over a screen-share.

Today all of those collapse into the same channel. The voice
parser does its best with whatever transcription comes out and
fires the highest-confidence intent. The user finds out what just
got attributed to them by watching it happen.

Bystanders fixes that by making attribution **visible**. Every
transcribed utterance gets a per-voice presence on screen. Real
users (whose fingerprints the system has, who are present in the
session) get weighted contributions. Everyone else's contribution
is shown but doesn't decide.

The design principle, said directly: **safety is a social
practice, not a software constraint.** The software offers tools
(audio attribution, vote primitives, hesitation detection, rules
files); the humans in the session use those tools to negotiate
with each other and with the system's defaults. The tool is the
accompanist, not the gatekeeper.

A second framing that shaped the design: **AI is a tool when it's
argued with, an authority when it's deferred to.** Pushback is the
feature, not the friction. The system should expect to be
redirected, refined, overruled. Anywhere the system would slide
toward authority — silently choosing for the user, presenting AI
predictions as if they were community sentiment, hiding what was
heard — is a design smell. The WALL-E framing: the way to not
become the Axiom is to build tools that keep the user *being a
user*.


## The visual model: dot stack, not pie chart

A pie chart says "here are the proportions of yes vs no." A dot
stack says "here is each voice I heard, individually, and which
way it leaned." The difference matters when there's noise.

A pie chart with one yes from the user and one yes from a radio
looks the same as two yes votes. A dot stack shows two distinct
dots, each its own color, and the user can see at a glance that
the radio chimed in too. The radio is harmless and visible. The
pie chart would have hidden it as aggregated noise; the dot stack
acknowledges it as a bystander.

### Layout

```
mobile / narrow:           desktop / wide:

    yes    no                 yes        no
    ()     ()                 ()()      ()
    ()     ()                 
    ()()                      
```

Dots cluster by vote direction. Same-direction dots overlap
slightly so they read as a cohesive group while staying
countable. On desktop the room is wider, so the cluster pairs
can lay horizontally. On mobile the cluster stacks vertically.

The overlap is doing real work. Fully separated dots read as
individuals (loses the "cluster" gestalt). Fully merged dots
lose the count. The slight overlap gives both — countable AND
grouped. This spacing carries meaning; don't tune it away.

### Color and shape

- **Recognized speakers**: solid color, the same color across
  sessions (assigned at hard-enrollment time, stored alongside
  their fingerprint).
- **Unrecognized speech (soft-enrolled)**: grey, slightly
  smaller, subtle border to read as "unknown person."
- **Detected non-speech (music, steady-state audio)**: where
  acoustic features let us tag it as such, a small music-note
  glyph or visual flicker. The radio shouldn't look like a
  person.
- **Uncategorized bucket** (see rate caps below): a single grey
  dot with a small `+N` notation when soft-enrollment overflow
  has collapsed several voices into "unknown plural."

### Interactions

Per-dot affordances, hover on desktop / tap on mobile:

- **Label**: shows the speaker name (or "guest 1" / "music?" /
  "unknown voices").
- **Audio playback**: tap-and-hold or right-click plays back
  the original audio clip that produced that dot. We have the
  samples (fingerprinting uses them); replay turns "I don't
  recognize that voice" into "oh, the podcast." Honest
  verification by ear.
- **Mute this source**: ignore further contributions from this
  voice for 30 seconds. The dot greys out, subsequent
  transcriptions from that fingerprint cluster get dropped.
  Direct manipulation of attribution — the user curates who
  influences this moment.
- **Promote to recognized**: if it's an unrecognized voice
  worth keeping, the user can tap "fingerprint as new speaker"
  and assign them a name + color. This is **hard enrollment**;
  see below.


## The gut rule: dedup by (speaker, direction)

When the same voice contributes multiple equivalent votes in the
same window, only the first one renders. Subsequent equivalent
votes get **gutted** from the display.

Two votes are equivalent if:

- **same speaker identity** (recognized speaker name, or same
  soft-enrolled fingerprint cluster)
- **same vote direction** (yes / no)
- **within the same vote window** (the deliberation period for
  this decision)

Allison says "yes" once, then "yes, definitely, yes I think so"
— still one dot. Allison says "yes" then "actually no" — two
dots, different directions, both shown. Radio steady-stream of
yeses — one dot from that unrecognized source. The pathological
case where VAD goes haywire and produces 30 transcriptions of
"yeah" in 2 seconds: one dot. The cluster stays readable at all
volumes of input.

The dedup is **visual only**. For tally purposes (real users'
votes counting toward quorum), each recognized speaker contributes
exactly one vote regardless of repetition. The gut rule simplifies
the *display*; it doesn't change the *decision*.

### Which dot survives

When equivalent votes arrive, the **first** one stays. The dot
snaps in at the moment of the first utterance and doesn't
reshuffle as repetitions arrive. Deterministic, matches how
humans remember sequences ("she said yes, then kept agreeing").


## Rate caps: hard cap on soft enrollment, dropped overflow

Even with the gut rule, a sudden burst of activity could create UI
churn (dots flying in, recomputing layout, sound effects) faster
than the user can read.

### Soft enrollment cap

Soft enrollment is implicit: when an unknown voice contributes,
the system fingerprints it on the fly, assigns a color, shows a
"guest" dot. The user didn't ask for this; the system did it.

**Hard cap: 3 soft enrollments per 2-second window. Overflow
drops into an "uncategorized" bucket.**

The uncategorized bucket is itself a single dot (the gut rule
applies to it like any other voice). It can carry a small `+N`
notation to indicate how many distinct unknown voices collapsed
into it.

The drop semantics are deliberate. Losing soft enrollments is
graceful — the user sees "3 known guests + 1 uncategorized
cluster" instead of "system is still processing, please wait."
Honest about what could and couldn't be separated. By the time a
queue would have drained, the moment has passed; retroactive
labeling doesn't help anyone.

Drop semantics also close off a small attack vector: a bad actor
scripting voice events can't flood the queue to delay enrollment
of a legitimate user joining at the same moment.

### Event cap

Already-recognized speakers contributing more votes also need a
pacing limit, even if each event after the first one is gutted by
the dedup rule. Without an event cap, a haywire VAD producing 30
transcriptions per second can still spike CPU and trigger layout
work even though nothing visible changes.

**Hard cap: 6 events per 2-second window across all sources.
Overflow is dropped.**

### Hard enrollment is uncapped

Hard enrollment is user-initiated: the user taps a guest dot and
says "this is Alice." The action has explicit human intent. It's
inherently rate-limited by the human's pace (one tap at a time).
No system cap is needed and a cap would be unhelpful — the user
might be deliberately enrolling several people in quick
succession.

### Tuning

All three values (soft cap = 3, event cap = 6, window = 2s) live
in the rules file (see below). Defaults chosen so a small meeting
(3-5 humans) all joining at once gets most people fingerprinted
within a couple of windows, while a chaotic burst stays bounded.


## Consensus changes pacing

When *one* recognized speaker is voting, the system responds
quickly — the user wants their action to land, the deliberation
window is short, the vote countdown is short.

When *multiple* recognized speakers are present and engaged, the
system slows down. Each speaker's request gets more time. The
deliberation window extends. The vote countdown lengthens.
Auto-execute thresholds tighten.

This is the inverse of typical responsiveness design. Usually
"faster is better." Here, **faster with one person, slower with
multiple, because the slow part is the deliberation itself, not
the system.** Rapid UI updates on every utterance in a
collaborative session prevent any one participant from being
fully read by the others. The system gets out of the way of the
conversation.

### Hesitation as deliberation signal

In multi-recognized-speaker sessions, voice signals of hesitation
— "wait," "um," "pero," equivalents in other languages — add 500ms
each to the active vote countdown. Hesitation is normally filler
to strip; in this specific context, it's a meaningful signal that
someone is still thinking and the system should give them more
time.

Gating to avoid misfire:

- Only triggers in the 2-second decision window (not anytime).
- Only triggers when the hesitating speaker is **not the one who
  initiated the action being voted on** (someone else is
  signaling hesitation about your action — that's the case worth
  paying attention to).
- Hard ceiling on extension: max 6 seconds of total extension per
  vote. A chatty session can't deadlock the action.
- Only active in **multi-recognized-speaker** mode. Solo
  sessions don't extend on hesitation — the user can pause
  whenever they like by just not voting.

### Solo session defaults

If one human is present, that human's vote constitutes quorum
immediately. The rules file can mark specific actions as "require
N humans" for genuinely dangerous things (`rm -rf $HOME`,
`force-push to main`, etc.) — those would block in a solo session,
which is the intended safety property.


## The fake-vote / Claude-prediction overlay

A speculative layer, **off by default**, that surfaces a
model-derived prediction of how votes might go — Claude saying
"based on the action and the context, I think most people would
vote no."

This is meaningfully different from the bystanders showing up in
the dot stack. The bystander dots represent **real audio caught
by the microphone, attributed honestly**. The Claude overlay
would represent **a model's opinion dressed as a vote-like
signal**. Those are not the same thing and the design treats
them differently.

### Why it's off by default

When Claude votes in your pie/dot stack, even as a 0-weight
visual nudge, it exerts a social pressure on the user's decision.
"I see Claude voted yes, I should probably vote yes too."
Multiplied across thousands of decisions, that's authority creep.

The WALL-E framing applies directly. The crew on the Axiom
stopped exercising their own judgment because the system was
always there with a smooth, never-wrong, never-arguing answer.
Off-by-default keeps the user in the driver's seat.

### What enabling it looks like

Enabling the overlay requires an explicit acceptance dialog. Not
buried in a settings panel — a real "are you sure" with plain
language:

> This shows Claude's prediction of how votes might go on
> decisions like this one. It's a social nudge. Reasonable
> people might think you should turn this off and exercise
> your own judgment. By turning it on, you accept that you're
> using AI as a social signal.

The warning isn't snark — it's exactly the trade-off, named
clearly. Then if the user wants it, they get it.

### Visual differentiation when enabled

Claude's dot is visually distinct from real bystander dots. A
different shape (a hollow ring instead of a filled circle), a
specific neutral color (not in the speaker color palette), and
a small icon glyph. The user should never confuse a Claude
prediction for a real voice. The label on hover/tap says "Claude
prediction" explicitly.

### Local-LLM variant

Same design with a local LLM substituted. Has the privacy
property of not sending audio off-device, which composes
naturally with the redact-in-meetings policies (see rules file
below) — anything the rules file says to redact from screen
share should also be redacted from the LLM prompt. Workshop-wall
semantics carry across surfaces: the rules file is the single
source of truth for "what stays private here."

Worth its own session to design the upload path — `voice-
pipeline.md` and the redaction policy need to be consulted
together. Not in scope for this doc.


## The rules file: `w2q.rules` — DEFERRED

**Status: shelved.** When this design was workshopped, the rules
file looked like the natural seat for per-directory safety
policy. On reflection the user pulled this back: "I am not so
sure about it and might just keep aggressive safety by default
for now."

That's the right call. The rules file would be authoring work
the user has to do *before* getting value, when what users
actually need first is sensible defaults that don't get in the
way. Forcing toml authoring on people learning the terminal is
exactly the kind of "tool becomes gatekeeper" move the design
philosophy elsewhere in this doc explicitly rejects.

The recorded sketch below stays as a reference for if the work
is ever picked up. The principle to preserve: **safety
defaults are aggressive, not configurable, until enough users
explicitly ask for configurability.** Until then, hardcode
the sensible behaviors and revisit when there's evidence the
hardcoding is wrong.

### What "aggressive safety by default" means concretely

- Dangerous-command list (rm -rf, force-push to main, etc.) is
  hardcoded with sensible block / confirm thresholds
- Redaction-in-meetings: detection of screen-share state is the
  trigger; the *what to redact* defaults to a small built-in
  pattern list (common key-file names, AWS-prefixed env vars,
  etc.)
- vim/helix wrappers: not added by default — power users have
  their own workflows here and a default wrapper would feel
  invasive

### Sketch preserved for reference

If/when this is revisited, the shape would be something like:

```toml
# Soft caps for the dot stack
[bystanders]
soft_enrollment_cap = 3
event_cap = 6
window_seconds = 2

# Per-action policies (would override defaults)
[rules.rm]
allow_paths = ["./build", "./target", "./dist"]

# Redaction
[redaction]
patterns = ["AWS_*", "api_key_*", "PRIVATE_KEY"]
default_mode = "blur_in_meetings"
```

The file would discover by walking up to repo root; rules would
layer most-specific-wins. None of this is committed; treat the
example as the design memory, not a schema.


## Shipping plan (provisional, not on active roadmap)

This section describes the cuts the work *would* land in if/when
it's picked up. It is **not committed**. Power-user table stakes
(`power-features.md`) outrank this for current development. The
plan stays here so picking it up later is mechanical rather than
re-design.

### Ship A — The dot-stack component

A QML Component that takes a stream of records:

```
{
  speaker_id: string,
  recognized: bool,
  color: color,
  vote: "yes" | "no",
  timestamp: int (ms),
  audio_ref: string,  // for playback
}
```

…and renders the dot cluster per the visual model above. The
component:

- Groups by (speaker_id, vote_direction) — applies the gut rule.
- Applies soft-enrollment cap to incoming records, collapses
  overflow into the uncategorized bucket.
- Applies event cap, drops overflow.
- Renders dots in the appropriate layout (desktop horizontal,
  mobile vertical).
- Wires up the per-dot affordances (label hover, audio playback,
  mute source, promote to recognized).

Built completely isolated. Fed by mock data or a debug command.
No integration with the live voice transcription path, no quorum
logic, no triggers. Just the visual primitive, working as a
standalone building block.

This is the seed. Everything else depends on it.

### Ship B — Wire to live voice

Connect the dot-stack to the real transcription pipeline. Each
utterance during an active vote window:

- Goes through speaker identification (already in place).
- Gets a `recognized: bool` flag based on fingerprint match
  confidence.
- Becomes a record passed to the dot-stack component.
- Counts toward quorum if the speaker is recognized.

Adds quorum logic: single-speaker session = one vote ends it,
multi-speaker session = wait for all recognized voices or
timeout. Adds the consensus-pacing rule (multi-speaker mode
extends the deliberation window, enables hesitation detection).

No new trigger paths yet. The vote is fired manually via a
debug command or voice command ("start a vote") to validate the
end-to-end flow.

### Ship C — Trigger paths

Connect actual command-execution paths to the vote primitive.
Initially:

- A hardcoded list of dangerous commands (e.g. `rm -rf`,
  `git push --force`) that always trigger a vote regardless of
  rules file.
- A `w2q.rules` reader that registers dynamic trigger rules.
- The voice-initiated "vote on this" command that triggers a
  vote for whatever's currently shown in the candidate picker.

This is where the design becomes user-visible as a feature. The
foundations from Ships A and B make this the smallest of the
three in implementation effort.

### Later ships (not in scope for this doc)

- Hesitation extension fully active (needs careful gating per
  rules above)
- Music / non-speech detection for the radio case
- Hard-enrollment promotion UX (the tap-to-add-speaker flow)
- Per-action rules file schema, finalized and documented
- Local LLM redaction-aware ingestion (whole separate arc; see
  voice-pipeline.md follow-up)
- Claude prediction overlay (opt-in, with the acceptance
  dialog)


## Open questions

Things we discussed but didn't fully resolve. Listed here so the
next implementor doesn't have to re-derive whether they're
settled or not.

### How long is the deliberation window?

We've been saying "2 seconds" for the countdown but haven't
pinned a value. Probably 2s solo, 4-5s multi-recognized-speaker,
6-12s with hesitation extensions. Needs live testing to tune.

### What happens when two unrecognized voices can't be told apart?

The fingerprinting can fail to distinguish two unrecognized
voices when both samples are short or low-quality. The design
says "merge into a single uncategorized cluster." That might be
wrong — the user might want to know there were multiple
bystanders. But pretending to distinguish them when we can't
is dishonest. The decision: show one dot, label clearly as
"unknown voices (plural)," let the user investigate via
playback if they care.

### Should the dot stack persist after the vote resolves?

Briefly, yes — at least a few seconds so the user can see what
just happened. A subtle fade or move-to-history affordance.
Persisting too long clutters the UI. Two seconds of decay,
maybe a "what just happened" log accessible via voice ("show
me the last vote").

### How does this interact with the existing voice picker?

The voice picker today is a candidate list. The dot stack is a
deliberation surface. They serve different moments — picker is
"what did I mean," dot stack is "should this happen." But they
might want to share visual language (colors, speaker
attribution) so users learn one and recognize the other.
Worth a session before implementing Ship A.

### When does an action become "vote-worthy"?

Currently designed as binary: either an action is in the
rules file as "require vote" or it's not. Could be more
nuanced — confidence-based, history-based, user-pattern-based.
The simple binary is enough for the first cuts; nuance can
come later.


## Names and framings to preserve

Things named explicitly during the design conversation that
should survive into the implementation. These aren't quotes —
they're design constraints.

- **"Voice is a canvas to paint on, not a menu to navigate."**
  Don't make the user fight suggestions. The system shows what
  it thinks; the user keeps painting.
- **"Don't limit creativity, but assist when getting in the way
  of the user."** The principle for when to add friction (only
  when the user is about to do something they'd regret) versus
  when to step back.
- **"Pushback is what turns AI into a tool and not authoritative
  controls."** The system should expect to be argued with.
  Anywhere it slides toward authority is a design smell.
- **"We don't want to end up like WALL-E."** The Axiom is the
  failure case. The way to avoid it is to keep the user
  exercising their own judgment.
- **"Each voice stays its own thing."** The dot-stack model in
  one sentence. No aggregation that hides who said what.

These are the load-bearing ideas. If a future implementation
choice would compromise one of them, that's a strong signal to
choose differently.
