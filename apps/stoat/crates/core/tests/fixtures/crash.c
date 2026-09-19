#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

struct node { int v; struct node *next; };

void present_frame(int i) { usleep(i % 7 == 6 ? 30000 : 4000); }

static int depth3(struct node *n) { return n->next->next != 0; }
static int depth2(struct node *n, int counter) { int doubled = counter * 2; return depth3(n) + doubled; }
static int depth1(int k) { struct node a = { k, NULL }; return depth2(&a, k); }

int main(int argc, char **argv) {
  for (int i = 0; i < 30; i++) {
    char *leak = malloc(1 << 20);
    memset(leak, i, 1 << 20);
    present_frame(i);
  }
  printf("argc=%d arg1=%s env=%s\n", argc, argc > 1 ? argv[1] : "-", getenv("STOAT_T") ? getenv("STOAT_T") : "-");
  fflush(stdout);
  return depth1(argc);
}
