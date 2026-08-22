# Roster balance: the pooled baseline

Numbers on one 160-game arm cannot resolve a kit-sized effect. Two independent
draft sets disagreed by twenty-five points on the same kit (Riptail 66% then
39%), because at ~70 appearances a kit's standard error is ±6 and *which drafts
it lands in* swamps that again. So the standing baseline is pooled.

## Protocol

```sh
for seed in 1 2 3 4 5; do
  cargo run --release --example battleground_balance -- 4 20 nowombo kit $seed
done
```

Five independent draft sets, 160 games each: **800 matches, 4,800 kit
appearances**, ~370 appearances per kit, ±2 standard errors ≈ ±5 points. Each
draft is played twice with the sides swapped on the same land, so a side
advantage cancels within every pair; each ancestor is credited for every game it
appears in, so a kit's number is not a statement about its teammates.

Runtime: about eight minutes for the full 800 after the jungle-cache fix (the
sim runs at ~28k steps/s; it was ~12k, and the same run cost nineteen minutes).

## Baseline — 800 matches, all thirteen kits

| kit | record | rate | |
|---|---|---|---|
| Windclaw | 202/364 | 55% | * |
| Rainspots | 203/366 | 55% | * |
| Stillsmoke | 207/394 | 53% | |
| Nightpelt | 194/372 | 52% | |
| Faryowl | 179/352 | 51% | |
| Riptail | 184/362 | 51% | |
| Duskstripe | 174/348 | 50% | |
| Suneye | 187/374 | 50% | |
| Goldenmane | 181/370 | 49% | |
| Stonewhisper | 167/354 | 47% | |
| Twofang | 166/354 | 47% | |
| Oldroar | 183/394 | 46% | |
| Snowhush | 173/396 | 44% | * |

`*` = outside two standard errors. Dispersion **sd 3.3**, range 12 points.

## After the mechanism hunt (same protocol, 800 matches)

The three outliers above were each traced to something the kit got for free, and
each fix is a rule rather than a number:

* **Windclaw 55% -> 49%.** Skyward Hunger quickened its paws on *every* kill —
  and the prey rework had started counting mice. A caracal could stack its whole
  speed bonus off a hillside of things that do not fight back. Prey now credits
  `prey_taken`; only warriors credit `kills`.
* **Rainspots 55% -> 53%.** Riverborn mended 3 hp/s in water with no cost, no
  cooldown and no cap, so the kit could stand in the shallows and outlast
  anything that came to the bank. The river closes wounds *between* fights now —
  not with an enemy inside engage range.
* **Snowhush 44% -> 48%.** The one significant underperformer, and its passive
  was value it handed to packmates the AI never spent: Hush of Snows chills, and
  nothing in the lynx's own kit collected on a chilled target. It does now
  (+25% into the slowed or rooted), which is the same teamwork idea stated so
  that a kit can pay itself.

| | before | after | delta |
|---|---|---|---|
| Windclaw | 55%* | 49% | −7 |
| Rainspots | 55%* | 53% | −3 |
| Snowhush | 44%* | 48% | +5 |
| dispersion | sd 3.3, range 12 | sd 3.3, range 13 | |
| kits outside 2se | 3 | **2** | |

Note what did *not* happen: dispersion did not improve. Three targeted fixes,
800 games each side, and the roster is exactly as spread as it was — the kits
outside two standard errors went 3 to 2, and Faryowl and Duskstripe drifted up
into the space Windclaw left. That is the honest shape of balance work at this
resolution: the fixes were right on their mechanisms, the aggregate barely
moved, and anyone reading a ±5 point delta as a result should remember that two
of these three deltas are inside their own error bar.

## Focus fire, and why the spread going UP was the good outcome

The flat aggregate above had a suspicious explanation, and it turned out to be
the right one: **the AI was hiding the roster.** Warriors picked the nearest
visible enemy, so three cats routinely chewed three different healthy targets
and killed nobody. The harness measures this directly — a draft pair is played
both ways, and if the same *composition* wins both times, composition decided
it; otherwise the chaos of first contact did. Before: **44%** of pairs were
composition-decided. Over half of all matches were being settled by which
orientation the dice favoured.

Warriors now prefer the enemy closest to falling, trading up to eight lengths
of distance for it (`FOCUS_WEIGHT`) — the patrol turns on the one already
bleeding. Result over 800 games:

| | before | focus fire |
|---|---|---|
| composition-decided pairs | 44% | **54%** |
| dispersion | sd 3.3 | sd 4.5 |
| kits outside 2se | 2 | 3 |

Dispersion rose, and that is not a regression. A roster cannot express itself
through an AI that spreads its damage at random; the earlier flatness was
partly noise standing in for balance. The honest reading is that the *new*
spread contains more signal than the old one did, and that every kit-level
number measured before this change was measured through a duller instrument.

Anything comparing kit strength should be re-run under focus fire. The
before-and-after tables above are kept as they were taken — they were true of
the AI that produced them.

## Counting the odds — the flattest roster so far, and this time it means it

The other half of nerve was missing. A warrior broke off when *wounded* and
never when *outmatched*, so cats at full strength walked into three enemies and
died there, over and over. Nerve is not only about how hurt you are; the books'
patrols withdraw from a bigger patrol. Warriors now count what they can see
within sixteen lengths and break off at a margin of two (one flaps: the AI
oscillates as a packmate crosses the radius, and a patrol that cannot commit is
worse than one that loses). Veiled cats do not count — you cannot fear what you
cannot see.

| | focus fire | + odds |
|---|---|---|
| dispersion | sd 4.5 | **sd 2.9** |
| range | 17 | **10** |
| kits outside 2se | 3 | **1** |
| composition-decided pairs | 54% | 54% |

That last row is what makes this different from the old flatness. Dispersion
halved while composition-determinism *held*, so this is not the AI hiding the
roster again — matches are still decided by who was drafted as often as before,
and the kits are simply closer together when played competently. The kits that
gained most are the ones that were dying for nothing: Oldroar +7 (the slow hulk
stops walking into threes it cannot escape), Snowhush +8, Riptail +6.

Every kit now sits within about five points of even, one of thirteen outside
two standard errors, on 800 games.

## Collapsing on a breach: correct play that measured worse

Third AI lever from the queue, and the one that did not pay. When a clan fells a
Heal Tree, its healthy warriors abandon their intact trails for the hole —
which is simply what a conquest team does, and which the sim previously only did
for patrol spawns and respawning cats, so a clan could break a border and then
keep walking three separate lanes past it.

| | odds | + collapse |
|---|---|---|
| dispersion | sd 2.9 | sd 4.2 |
| kits outside 2se | 1 | 2 |
| composition-decided pairs | 54% | 52% |

Flat-to-worse on both metrics over 800 games, so it lives behind
`Battleground::collapse_on_breach`, defaulting **off**. The likely reason is
visible in the rule itself: *every* healthy warrior collapses, so the clan
empties its other two lanes and trades a border for two borders. A version that
leaves one cat holding should get a different answer — and the switch is how
that gets tested rather than argued.

Both behaviour switches (`wombo`, `collapse_on_breach`) default off for the same
reason and can be turned on per run:

```sh
cargo run --release --example battleground_balance -- 4 20 wombo kit 1 push
```

## A push that leaves a holder, and a metric for the AI itself

The all-in collapse above was diagnosed as an abandonment, so the switch now
sends everyone *except* the lowest seat on each lane — a push with somebody
holding. That diagnosis was right and the repair partial:

| | no push | all-in | holder |
|---|---|---|---|
| dispersion | sd 2.9 | sd 4.2 | sd 3.5 |
| kits outside 2se | 1 | 2 | 2 |
| composition-decided | 54% | 52% | 54% |

Composition-determinism came back to 54% and most of the dispersion damage
undid itself, but pushing still does not beat not pushing. Default stays off.

That prompted a third metric, because dispersion and composition-determinism
both measure the *roster*, and nothing measured the AI: **does the clan that
breaks the enemy's first border go on to win?** A team that takes the first
objective and loses anyway is not converting an advantage.

The answer was **43%** pooled over 320 games — breaking a border was not an
advantage, it was slightly a liability. Reading the sim explained it: felling a
tree costs time and blood deep in enemy ground and bought almost nothing,
because the Great Tree's root guard was all-or-nothing. Two standing trees
hardened it exactly as much as three. So the guard now scales — each felled
border is a third of it gone, tested directly (`every_felled_border_softens_the_great_tree`).

**And the conversion rate did not move: 48% and 39%, the same two seeds, before
and after.** The mechanism is right and evidently was not the binding
constraint. Left open rather than papered over; the likeliest remaining
explanation is that the breaching clan simply overextends and pays for the
border in bodies, which is a question for the retreat rules rather than the
structures.

## Where the border advantage actually went

`examples/breach_probe.rs` asks what happens in the minute after the first Heal
Tree falls. The overextension theory — the breacher dies deep in enemy ground
paying for its prize — is **wrong**: deaths were even (21 against 25) and at
identical depth (103 m from each cat's own camp). Nor is breaking a symptom of
losing; at the moment of the break the breacher was *ahead* on freshkill in
17 of 29 matches.

What the probe did find was in the end state: the breacher finished with **fewer
of its own borders standing** (−0.28). It had traded the map for a lane —
because `breach_lane` funnelled *every* patrol wave into the hole it had made,
while the defender, having no breach of its own, kept covering three trails.
Concentration is right; total concentration is a gift.

Half the waves press the breach now, the rest walk the rota. The mechanism
metric moved almost to neutral (**−0.28 → −0.07** borders-standing lead) and the
probe's conversion went 34% → 41%.

**The conversion rate is still not fixed.** Pooled over 793 harness games it
sits at 40.9% (se 1.8) — five standard errors below even, so defending remains
genuinely stronger than attacking here, and two of the three explanations tried
have now been eliminated rather than confirmed. Candidates left: the defender
fights beside its medic den and inside its own auras, respawns closer, and gets
the last-stand rule that suspends retreat at the Great Tree. That is a real
asymmetry in the mode's design rather than a bug, and worth deciding
deliberately — a MOBA usually pays the attacker for the risk, and this one does
not yet.

## What this says, and what it does not

Ten of thirteen kits are statistically indistinguishable from even. The three
that are not — Windclaw and Rainspots at 55%, Snowhush at 44% — are each about
two standard errors out, which is the weakest kind of real. They are worth a
mechanism hunt, not a number nudge: every balance problem found so far turned
out to be something a kit got *for free* (a kite that cost no attack, a veil
that survived the blow, a shield that never expired), and each of those was
visible by reading the sim rather than the table.

Single-arm numbers are still printed with their error bars, and should be read
as noise unless they clear it. A kit-strength claim wants this whole protocol
re-run, not one more arm.
