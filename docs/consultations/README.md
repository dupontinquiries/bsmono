# Consultations

First-hand accounts from people who know a field, kept as files.

A run raises a consultation when it hits a question that is (a) outside
its competence, (b) load-bearing, and (c) one it is not confident about —
see `logscope::consult::should_consult`. Raising one **never blocks**: the
run records what it would ask, records what it decided anyway, and carries
on. An answer arriving later is applied by branching from the recorded
message index, not by patching the end.

An unanswered consultation stops the run after 100 messages. That is not a
politeness timer — it is the derailment alarm. A hundred messages built on
an unchecked assumption is the failure this is for.

## Answering one

Edit the file. Fill in the `## First-hand account` section with your own
words — verbatim, not summarised, because a paraphrase of a first-hand
account is a second-hand one — and set `status: answered` plus `expert:`
in the front matter.

Your `standing` line is why you are worth asking: "wrote the firmware",
"PhD, numerical analysis", "ran this rig for six years". It is free text
because credentials do not enumerate.

## Why files

Because the probe layer already reads files. `Grep` and `Slice` reach
`docs/consultations/**/*.md` with no new machinery, so an expert's answer
joins the same pool as the source — searchable, attributable, dated and
diffable. A transcript is none of those things.
