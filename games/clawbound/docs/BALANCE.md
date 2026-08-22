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
