# Catalogue categories — what runs, and what needs your engine

## Run this

    python3 install_models_v5.py \
        --iqm-dir iqm --parts-dir kit \
        --engine /path/to/engine-cmake-project/engine_src \
        --tuning model_tuning.json \
        --exclude-file exclude-ip.txt \
        --fixed-list fixed.txt \
        --forge-dir /path/to/c19/games/blackstar-rs/assets/forge \
        --dry-run

Drop `--dry-run` when the counts look right.

## What it does without any engine change

The generated header has no category field and nothing in `game.c` reads one,
so the catalogue is one flat alphabetical list of `g_imported_names`. Adding a
real category means a UI change.

This gets grouping anyway: the category becomes a **prefix on the display
name**, and since the list is already sorted by name, the prefix groups it.

    FORGE ANVIL
    FIXED HEAVY GATLING TURRET
    OLD   1967 CHEVROLET CAMARO RS
    IP    04B <licensed name, redacted>

Sort order is category-first, then name, so the four blocks are contiguous.

Width is spent on the prefix deliberately. Names already truncate at 28 in
your screenshot; knowing which bucket a model is in matters more than its last
four characters.

## What it also emits, for when the UI catches up

    #define EX_N_CATEGORIES 4
    static const char *g_imp_cat_names[EX_N_CATEGORIES] = { "FORGE","FIXED","OLD","IP" };
    static const unsigned char g_imp_cat[EX_N_IMPORTED] = { 2, 3, 2, ... };

Unused arrays are harmless in C. When `game.c` grows a category filter, the
data is already there and already correct — no re-run.

## Measured against your actual installed set

1,009 models from `imported_models.h`, 122 patterns from `exclude-ip.txt`:

    FORGE      0   nothing generated is installed yet
    FIXED      1   re-exported with texture_export v13+
    OLD      456   as previously installed
    IP       552   filename matches a rights holder

**552 of 1,009 — 55% — are rights-holder material and live in the catalogue
right now.** Five licensed marks (franchise, vehicle and mecha names; redacted here).

This tool LABELS. It does not prune. Everything still installs; the IP block
is just visible instead of scattered through the list.

## Two things I could not do

**Patch the catalogue UI.** Your build says 1164 entries; the
`imported_models.h` in c19 says `EX_N_IMPORTED 1012`, and the string
"CATALOGUE" does not appear in my copy of `game.c` at all. Your engine is
ahead of the checkout I have, so a UI patch from me would be written against
source I cannot see.

**Actually re-export the old models.** That needs the model library, which is
not in the repo. The fix is proven on two folders:

    Basic Rocket Engine    v9  3,860 tris   v13  45,640 tris   v9 kept 8%
    Heavy Gatling Turret   v9  3,618 tris   v13  38,958 tris   v9 kept 9%

`texture_export_v9` takes whichever `.obj` `os.listdir` returns first and
ignores the rest — a single `break` in the walk. v13 merges them all. Five
folders in the library are multi-file; the rest are unaffected, which is why
`Rocket_BM21-122mm` (one vertex block) came out byte-identical from both and
would have "proved" v9 fine.

To produce `fixed.txt`, re-run v13 and list what changed:

    python3 texture_export_v13.py . --out-iqm parts --out-tex parts/textures \
            --cap 1024 --tex-format auto --force
    # every folder whose output grew is a FIXED entry
