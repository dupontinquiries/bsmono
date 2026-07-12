#include "gates.h"
#include <string.h>

void ex_gates_init(ex_gate_t gates[EX_GATE_COUNT]){
  memset(gates, 0, sizeof(ex_gate_t) * EX_GATE_COUNT);
  gates[0].cost = 300;  gates[0].extent = 100.0f; gates[0].name = "the outer causeway";
  gates[1].cost = 750;  gates[1].extent = 150.0f; gates[1].name = "the reclaimed foundry wing";
  gates[2].cost = 1500; gates[2].extent = 220.0f; gates[2].name = "the outer rim causeway";
}

int ex_gate_try_open(ex_gate_t *g, long *energon){
  if (g->opened) return 0;
  if (*energon < g->cost) return 0;
  *energon -= g->cost;
  g->opened = 1;
  return 1;
}

int ex_gates_next_locked(const ex_gate_t gates[EX_GATE_COUNT]){
  for (int i = 0; i < EX_GATE_COUNT; i++)
    if (!gates[i].opened) return i;
  return -1;
}

float ex_gates_extent(const ex_gate_t gates[EX_GATE_COUNT]){
  float ext = 60.0f;   // starting arena half-extent
  for (int i = 0; i < EX_GATE_COUNT; i++)
    if (gates[i].opened && gates[i].extent > ext) ext = gates[i].extent;
  return ext;
}
