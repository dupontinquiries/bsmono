# Example conversations & scripts

Run these from the **suite root** so the relative paths resolve. Full guide: `docs/10-cli.md`.

```bash
cargo build --bin logscope-cli --no-default-features          # build once
logscope-cli --mock examples/02-lru-fill.logscope             # try with the mock
logscope-cli --model ~/models/<your>.gguf examples/01-sort-generics.logscope > runs/01.md   # real, on the M2
```

## Scripts (`.logscope`)

| file | exercises |
|---|---|
| `01-sort-generics.logscope` | a concrete impl → generalize → `:2ca` deepen → `:best` → `:export md` |
| `02-lru-fill.logscope` | `:open` a skeleton reply → `:fill` (one KV-shared sub-task per `todo!()`) → assemble |
| `03-fanout-explore.logscope` | `:2fa 3 '…{lastmsg}…'` — fuzzy-all, 3 variants/leaf × 2 rounds → `:best` |
| `04-parser-fill.logscope` | a second `:fill` target with a different shape (free fns, nested types) |

## Seeds (`.chat.md`, opened by the scripts)

`lru-skeleton.chat.md`, `parser-skeleton.chat.md` — a reply whose code block is a `todo!()` skeleton, the
input `:fill` decomposes. `sort-ints.chat.md` — a plain seed chat to `:open`.

## Add your own

A seed is just `## You` / `## Assistant` turns (what `:export md` emits, so runs round-trip back in via
`:open`). A script is comments, `>` user turns, and `:` commands. To turn a session you built in the GUI
into a repeatable test: `:export md` it, save under `examples/`, and write a short `.logscope` that
`:open`s it and runs the commands you want to regression-check.
