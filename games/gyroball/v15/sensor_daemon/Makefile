# Makefile for tilt_sensor daemon
#
# Three build targets:
#
#   make              — tries pkg-config libiio first, falls back to submodule
#   make submodule    — build libiio from git submodule (no apk/apt needed)
#   make sysfs        — pure sysfs, zero dependencies
#
# PostmarketOS / Alpine setup:
#   git submodule add https://github.com/analogdevicesinc/libiio.git libiio
#   apk add cmake build-base libxml2-dev libudev-zero-dev
#   make submodule
#
# Debian / Ubuntu setup (if libiio-dev is packaged):
#   apt install libiio-dev gcc make
#   make
#   -- or submodule if not packaged --
#   git submodule add https://github.com/analogdevicesinc/libiio.git libiio
#   apt install cmake build-essential libxml2-dev libudev-dev
#   make submodule

CC      = gcc
CFLAGS  = -O2 -Wall -Wextra
LIBIIO_SUBMOD = libiio
LIBIIO_BUILD  = libiio/_build
LIBIIO_INC    = $(LIBIIO_SUBMOD)
LIBIIO_LIB    = $(LIBIIO_BUILD)/libiio.a

.PHONY: all submodule sysfs clean distclean

# ── Default: use system libiio if available, else guide user ──────────────────
all:
	@if pkg-config --exists libiio 2>/dev/null; then \
		echo ">>> Using system libiio"; \
		$(CC) $(CFLAGS) -o tilt_sensor tilt_sensor.c \
			$$(pkg-config --cflags --libs libiio); \
		echo "Built tilt_sensor (system libiio)"; \
	elif [ -f "$(LIBIIO_LIB)" ]; then \
		echo ">>> Using submodule libiio"; \
		$(CC) $(CFLAGS) -I$(LIBIIO_INC) -o tilt_sensor tilt_sensor.c \
			$(LIBIIO_LIB) -lxml2 -lpthread -ldl; \
		echo "Built tilt_sensor (submodule libiio)"; \
	else \
		echo ""; \
		echo "libiio not found. Options:"; \
		echo "  1) Build from submodule (no package needed):"; \
		echo "       git submodule add https://github.com/analogdevicesinc/libiio.git libiio"; \
		echo "       make submodule"; \
		echo "  2) Zero-dependency sysfs fallback:"; \
		echo "       make sysfs"; \
		echo ""; \
		exit 1; \
	fi
	@echo "Run: ./tilt_sensor | python3 ../ball_maze.py [username]"

# ── Build libiio from git submodule, then link statically ─────────────────────
submodule: $(LIBIIO_LIB)
	$(CC) $(CFLAGS) -I$(LIBIIO_INC) -o tilt_sensor tilt_sensor.c \
		$(LIBIIO_LIB) -lxml2 -lpthread -ldl
	@echo "Built tilt_sensor (static libiio)"
	@echo "Run: ./tilt_sensor | python3 ../ball_maze.py [username]"

$(LIBIIO_LIB): $(LIBIIO_SUBMOD)/CMakeLists.txt
	mkdir -p $(LIBIIO_BUILD)
	cd $(LIBIIO_BUILD) && cmake .. \
		-DBUILD_SHARED_LIBS=OFF \
		-DWITH_TESTS=OFF \
		-DWITH_DOC=OFF \
		-DWITH_MATLAB_BINDINGS=OFF \
		-DWITH_PYTHON_BINDINGS=OFF \
		-DCMAKE_BUILD_TYPE=Release
	$(MAKE) -C $(LIBIIO_BUILD) iio

$(LIBIIO_SUBMOD)/CMakeLists.txt:
	@echo "libiio submodule not initialised. Run:"
	@echo "  git submodule add https://github.com/analogdevicesinc/libiio.git libiio"
	@echo "  git submodule update --init libiio"
	@exit 1

# ── Sysfs fallback — pure C, no deps ─────────────────────────────────────────
sysfs: tilt_sensor_sysfs.c
	$(CC) $(CFLAGS) -o tilt_sensor_sysfs tilt_sensor_sysfs.c
	@echo "Built tilt_sensor_sysfs (sysfs, no libiio)"
	@echo "Run: ./tilt_sensor_sysfs | python3 ../ball_maze.py [username]"

clean:
	rm -f tilt_sensor tilt_sensor_sysfs

distclean: clean
	rm -rf $(LIBIIO_BUILD)
