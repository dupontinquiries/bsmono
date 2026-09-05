# Example Makefile

# -- Dirs -- #
CC      =gcc
EDIR    =exengine
LDIR    =lib
BDIR    =build
BNAME   =game
ODIR    =obj
IDIR    =inc
EDATA   =exengine
DATA    =data
_IDIRS  =lib inc lib/physfs lib/MojoAL lib/SDL2 exengine
IDIRS   =$(patsubst %,-I%/,$(_IDIRS))

# -- Flags -- #
FLAGS   =-g -lm -ldl -Wall -Wno-unused -Wno-uninitialized -lstdc++ \
-lGL -lSDL2 -I. $(IDIRS) '-Wl,-z,origin' '-Wl,-rpath,$$ORIGIN/lib'
#
CFLAGS  =$(FLAGS)
CFLAGS +=-std=c99 -O2

# -- Windows -- #
# -- Windows: removed --
#
# The mingw cross-compile and its 17 MB of prebuilt SDL2 under lib/win are gone.
# This targets Linux, with macOS as a bonus; both link SDL2 from the system
# rather than from a vendored archive, so neither needed lib/win and nothing
# outside this branch referenced it.
#
# Restoring it means: this branch back, `-Llib/win -lSDL2main -lSDL2`, an
# x86_64-w64-mingw32 toolchain, and SDL2's own mingw development release —
# which is a download, not something that has to live in this repo's history.

# -- MacOS -- #
UNAME   = $(shell uname -s)
ifeq ($(UNAME),Darwin)
FLAGS   =-g -lstdc++ -framework OpenGl -framework Foundation \
-framework IOKit -lSDL2 \
-I. $(IDIRS) -Wno-unused-command-line-argument
#
CFLAGS  =$(FLAGS)
CFLAGS +=-std=c99 -O2
endif

# -- OpenBSD -- #
UNAME = $(shell uname -s)
ifeq ($(UNAME),OpenBSD)
CC =egcc
FLAGS += -I/usr/X11R6/include -L/usr/X11R6/lib
FLAGS:=$(filter-out -ldl,$(FLAGS)) #-ldl functionality provided in OpenBSD libc
endif

# -- Files -- #
# engine deps
_EDEPS_EN = engine.h
# input
_EDEPS_IN = input.h scancodes.h
# math
_EDEPS_MA = math.h collision.h mathlib.h octree.h
# render
_EDEPS_RE = render.h scene.h camera.h mesh.h model.h msdf.h \
pointlight.h shader.h stb_image.h stb_truetype.h text.h \
texture.h window.h vga.h vga_font.h renderer.h
# sound
_EDEPS_SO = sound.h
# util
_EDEPS_UT = util.h cache.h entity.h ini.h io.h list.h iqm.h \
vertices.h
EDEPS   =$(patsubst %,$(EDIR)/%,$(_EDEPS_EN))
EDEPS  +=$(patsubst %,$(EDIR)/input/%,$(_EDEPS_IN))
EDEPS  +=$(patsubst %,$(EDIR)/math/%,$(_EDEPS_MA))
EDEPS  +=$(patsubst %,$(EDIR)/render/%,$(_EDEPS_RE))
EDEPS  +=$(patsubst %,$(EDIR)/sound/%,$(_EDEPS_SO))
EDEPS  +=$(patsubst %,$(EDIR)/util/%,$(_EDEPS_UT))

# engine srcs
ENGINEOBJ = window.o glad.o mesh.o camera.o texture.o iqm.o \
pointlight.o scene.o model.o collision.o entity.o octree.o \
engine.o shader.o input.o sound.o cache.o text.o msdf.o \
ini.o vga.o renderer.o \
ability.o absorb.o ally.o arc.o arena.o behavior.o bsgames_handoff.o campaign.o chassis.o coll_chunk.o combat.o combat_json.o commander_ai.o containment.o corrupt.o cyclone.o detail.o element.o encounter.o enemy.o events.o feel.o gates.o graviton.o hive_ai.o keybind.o lore.o mapfsm.o mapgen.o material.o matrix.o menu.o net_transport.o netcode.o pclass.o photon.o pickup.o provenance.o resonance.o robot_parts.o signature.o siphon.o spearcon.o terrain_step.o thorns.o transform.o truce.o tsunami.o turret.o vehicle.o vfx.o voice.o warp_wave.o waves.o weapon.o weaponmod.o

# lib deps
_PHYSFS_DEPS =physfs_casefolding.h  physfs.h  physfs_internal.h \
physfs_lzmasdk.h  physfs_miniz.h  physfs_platforms.h
PHYSFS_DEPS  =$(patsubst %,$(LDIR)/physfs/%,$(_PHYSFS_DEPS))

_MOJOAL_DEPS =alc.h al.h
MOJOAL_DEPS  =$(patsubst %,$(LDIR)/MojoAL/AL/%,$(_MOJOAL_DEPS))

# lib srcs
LOBJ    = physfs_archiver_7z.o physfs_archiver_mvl.o \
physfs_archiver_wad.o physfs_platform_posix.o \
physfs_archiver_dir.o physfs_archiver_qpak.o \
physfs_archiver_zip.o physfs_platform_qnx.o \
physfs_archiver_grp.o physfs_archiver_slb.o \
physfs_byteorder.o physfs_platform_unix.o \
physfs_archiver_hog.o physfs_archiver_unpacked.o physfs.o \
physfs_platform_windows.o physfs_archiver_iso9660.o \
physfs_archiver_vdf.o physfs_platform_os2.o physfs_unicode.o \
mojoal.o

ifeq ($(UNAME),Darwin)
LOBJ += physfs_platform_apple.o
endif

# usr srcs (add yours here)
_OBJ    =main.o game.o
_OBJ   +=$(LOBJ)
_OBJ   +=$(ENGINEOBJ)
OBJ     =$(patsubst %,$(ODIR)/%,$(_OBJ))

# user deps
_DEPS   =game.h
DEPS    =$(patsubst %,$(IDIR)/%,$(_DEPS))

all: files game

# user src
$(ODIR)/%.o: %.c $(DEPS)
	$(CC) -c -o $@ $< $(CFLAGS)

# engine srcs
$(ODIR)/%.o: $(EDIR)/%.c $(EDEPS)
	$(CC) -c -o $@ $< $(CFLAGS)
$(ODIR)/%.o: $(EDIR)/*/%.c $(EDEPS)
	$(CC) -c -o $@ $< $(CFLAGS)

# physfs src
$(ODIR)/%.o: $(LDIR)/physfs/%.c $(PHYSFS_DEPS)
	$(CC) -c -o $@ $< $(CFLAGS)

ifeq ($(UNAME),Darwin)
$(ODIR)/%.o: $(LDIR)/physfs/%.m $(PHYSFS_DEPS)
	$(CC) -c -o $@ $< $(CFLAGS)
endif

# mojoal src
$(ODIR)/%.o: $(LDIR)/MojoAL/%.c $(MOJOAL_DEPS)
	$(CC) -c -o $@ $< $(CFLAGS)

game: $(OBJ)
	$(CC) -o $(BDIR)/$(BNAME) $^ $(CFLAGS)
	@echo "**success**"

files:
	mkdir -p $(ODIR)
	mkdir -p $(BDIR)/licence
	cp licence/* $(BDIR)/licence/
	(cd $(EDATA); zip -ur ../build/data.ex $(DATA) || true)
	(zip -ur build/data.ex $(DATA) || true)

release-linux:
	mkdir -p $(BDIR)/lib
	rm -f $(BDIR)/lib/*
	cp $$(ldd $(BDIR)/game | grep -i libopenal | awk 'NF == 4 {print $$3}; NF == 2 {print $$1}') $(BDIR)/lib
	cp $$(ldd $(BDIR)/game | grep -i libsdl2 | awk 'NF == 4 {print $$3}; NF == 2 {print $$1}') $(BDIR)/lib

#release:
#ifeq ($(OS),Windows_NT)
# copy /b $(BDIR)/game.exe+$(BDIR)/data.exe $(BDIR)/release.exe
#else
# cat $(BDIR)/game $(BDIR)/data.ex > $(BDIR)/release
# chmod +x $(BDIR)/release
#endif

.PHONY: clean release-linux

clean:
	rm -f $(ODIR)/*.o
