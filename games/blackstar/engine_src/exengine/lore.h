/* lore
  An in-game codex: short, ORIGINAL entries that carry the creative vision
  across the six-film arc (two trilogies) without reproducing any source text.
  Each entry is a title + a few wrapped lines. Purely static data; testable.
*/
#ifndef EX_LORE_H
#define EX_LORE_H

#define EX_LORE_MAX_LINES 6

typedef struct {
  const char *title;
  const char *lines[EX_LORE_MAX_LINES];
  int line_count;
} ex_lore_entry_t;

int ex_lore_count(void);
const ex_lore_entry_t* ex_lore_entry(int i);

#endif // EX_LORE_H
