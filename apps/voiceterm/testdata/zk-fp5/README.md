# zk-fp5 — a Fairphone 5 demo vault

Eleven interlinked notes built from *Fairphone 5: Information on how to repair
and recycle* (95 pp, rev. 2025-07-23). Drop the directory into a vault root
and open `202310150900 Fairphone 5.md`.

Timestamp ids (`YYYYMMDDHHMM`), wikilinks, no frontmatter — the plain form
the note graph already expects.

## What each note is for

| Note | Demo | Test surface |
|---|---|---|
| 202310150900 Fairphone 5 | the hub | hub-and-spoke backlinks; every other note links up to it |
| …0901 hardware specification | five tables in one note | table rendering, long rows, mixed units |
| …0902 sensors and the LPI | a graph with five shapes | niftyplot graph kind, labelled edge, `rect/circle/hexagon/cylinder/diamond` |
| …0903 disassembly flow | a 12-node DAG, 12 edges | force-directed layout under branching |
| …0904 spare parts and screws | four small tables | short tables in sequence |
| …0905 recycler material mass | a 21-row table plus a graph | the widest table; two real data errors left in place |
| …0906 diagnostics | grouped table | cells with long comma lists |
| …0907 mainline Linux gaps | table + graph | a note that is half prose, half diagram; the graph is 11/8 |
| …0909 camera pipeline | measured device findings | tables plus shell blocks; the only note with runnable commands |
| …0908 capture damage sample | broken on purpose | `docingest::diagnose` — empty refs, base64 asset names, split table cells |
| …0910 deploy tideshell and COSMIC by hand | a runbook: numbered stages, shell blocks, an unverified section | code blocks in notes; a procedure as a note |

## Test suite notes

- **Link graph**: 10 files, 9 notes plus this README. The hub carries 9
  wikilinks; every other note links back up to it, and 0902↔0907, 0903↔0904,
  0905←0908 and 0906→0907 are the lateral edges, so `neighbors()` returns
  more than the hub for those.
- **docingest**: measured with `diagnose`, 0908 is the only note that scores
  (1.0). It reports 9 empty refs and 3 base64 asset names. Every other note
  scores 0.0 — a repair pass that flags one of them is a false positive worth
  chasing.
- **A known detector gap, deliberately left in**: 0908 also contains a table
  whose cells are split across lines, and `ragged_table_rows` reports **0**
  for it. The split-cell form is one of the four damage classes docingest
  documents, and the current check does not catch this instance. That makes
  0908 a regression test for a fix that has not been written yet.
- **niftyplot**: four graph-kind blocks, parsed with `niftyplot::parse` —
  0902 (7 nodes, 6 edges), 0903 (12/12), 0905 (10/7), 0907 (11/8). All four
  parse with zero warnings and zero errors. 0902 exercises the labelled-edge
  form (`A -- "label" → B`); the others are plain chains.
- **Data quality**: 0905 contains two genuine errors from the source document
  (one CAS number used for three metals, cobalt listed twice with different
  figures). They are documented in the note itself, so a reader who spots
  them finds the explanation rather than filing a bug.
