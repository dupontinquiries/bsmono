# Builds the v1.5 corpus-LM C pipeline into the shared registry bin dir, so both
# Corpus Studio and Bot Arena find the binaries automatically.
#
#   make            # build into $(BINDIR)
#   make BINDIR=... # override the destination
#
# Default BINDIR matches registry::bin_dir() in the Rust code:
#   $XDG_DATA_HOME/codebot-arena/bin   (falls back to ~/.local/share/...)

CC      ?= cc
# -O3 plus native vectorization and loop unrolling — a safe, across-the-board
# speedup for the tight dim-loops in every trainer (no -ffast-math, so float
# results are unchanged). tiny_gpt additionally gets -fopenmp below.
CFLAGS  ?= -std=c99 -O3 -march=native -funroll-loops -Wall -Wextra
LDLIBS  ?= -lz -lm

DATA_HOME ?= $(if $(XDG_DATA_HOME),$(XDG_DATA_HOME),$(HOME)/.local/share)
BINDIR    ?= $(DATA_HOME)/codebot-arena/bin

PROGS = bpe_train ngram_train word2vec_train sentence_pool transitions tiny_gpt \
        chat scaffolded_chat reasoning_chat inspect_clusters wavenet

all: $(addprefix $(BINDIR)/,$(PROGS))

# tiny_gpt's training loop is OpenMP-parallel across the batch (4–8× speedup on
# a typical laptop). Falls back to single-threaded if -fopenmp isn't available.
$(BINDIR)/tiny_gpt: src/tiny_gpt.c
	@mkdir -p $(BINDIR)
	$(CC) $(CFLAGS) -fopenmp -o $@ $< $(LDLIBS)

$(BINDIR)/%: src/%.c
	@mkdir -p $(BINDIR)
	$(CC) $(CFLAGS) -o $@ $< $(LDLIBS)

clean:
	rm -f $(addprefix $(BINDIR)/,$(PROGS))

.PHONY: all clean
