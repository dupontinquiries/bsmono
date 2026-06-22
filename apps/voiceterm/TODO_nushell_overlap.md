# TODO: nushell parity ("increase the overlap")

## Done (v0.1.635)
Closed two real gaps where nushell fell OUTSIDE the bash/zsh/fish handling:
1. **missing-package detection** (missing_packages.rs): nushell's
   command-not-found format is entirely different — `Command `<name>` not
   found` (in nu's box-drawing error display, name in BACKTICKS), not
   "<name>: command not found". Added a parser for it (guarded so the
   "help: Did you mean `x`?" suggestion line is NOT mistaken for the
   missing command). Verified format against nushell docs/issues. 3 tests.
2. **LLM-context noise** (main.rs ~14843): the "Currently running: <fg>"
   line was suppressed for bash/zsh/sh/fish but NOT nu — so nu users got a
   spurious "Currently running: nu" injected into every LLM context.
   Added nu/nushell to the exclusion.

## Still TODO (the bigger nushell vision from docs/todo.md ~3771)
User direction quoted there: "adding nushell nlp defaults and lexicon… it
really is a different shell." The doc sketches a per-shell NLP dialect
registry (bash.rs / nu.rs / fish.rs / python.rs) feeding table_nlp, where
nu.rs encodes structured-pipeline idioms (e.g. natural-language → `where
$it =~ "query"` instead of bash's pipe-to-grep). That dialect layer is NOT
built — table_nlp.rs is still the only NLP path and is bash-shaped.

Next pure slices (buildable + testable in-sandbox):
- A `shell_dialect` enum + detection (nu vs bash-family vs fish vs python)
  from foreground cmd + prompt — pure, like grammar_context.
- nu.rs lexicon: a handful of natural-language → nu-idiom templates
  (filter/where, select columns, sort-by, length/count) as pure
  string-template functions with tests. Structured pipelines + regex on
  $it, NOT bytes-to-grep.
- Then wire table_nlp to pick the dialect by detected shell.

Don't over-build the lexicon speculatively — start with the 4-5 idioms a
nu user hits most (where/select/sort/length/get) and grow from dogfood.

## v0.1.636 — shell_dialect detector DONE (first NLP-routing slice)
src/shell_dialect.rs: detect(foreground_cmd, prompt_line) -> Dialect
{BashFamily, Nushell, Fish, Python, Unknown}. Command name is the primary
signal (normalizes login "-bash" + path "/usr/bin/nu"); prompt rescues the
python REPL case. Key safety props (tested): Unknown does NOT assume bash
(caller keeps its generic path, no misrouting); substring names like
numfmt/nuget do NOT false-match nu; command beats prompt. Dialect::
is_structured() (true only for nu) is the one bit the NLP layer most
needs (filter → `where` vs `grep`). 8 tests.

### Next slice (pure, buildable in-sandbox): the nu lexicon
A `nu_lexicon` module: natural-language → nu-idiom string templates for
the 4-5 idioms a nu user hits most. Concrete starters:
- "filter/where X contains Y"  → `where $it =~ "Y"` (or `where COL =~`)
- "select columns A B"          → `select A B`
- "sort by COL"                 → `sort-by COL`
- "count/length"                → `length`
- "get COL / first N"           → `get COL` / `first N`
Pure string-template fns + tests (like the bash table_nlp templates).
THEN wire: table_nlp picks the template set by shell_dialect::detect — if
Nushell use nu_lexicon, else current bash path; Unknown → current path.
Don't speculatively over-grow the lexicon; start with these, expand from
dogfood. The detector makes the routing a clean, testable switch.
