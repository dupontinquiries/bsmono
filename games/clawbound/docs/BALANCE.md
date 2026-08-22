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
