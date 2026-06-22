# PhotoSync System Makefile

CARGO := cargo
INSTALL := install
PREFIX := /usr/local
DESTDIR :=

# Systemd directories
SYSTEMD_USER_DIR := $(PREFIX)/lib/systemd/user
SYSTEMD_SYSTEM_DIR := $(PREFIX)/lib/systemd/system

# Binary directory
BIN_DIR := $(PREFIX)/bin

# State directory
STATE_DIR := /var/lib/photosync

.PHONY: all build test clean install install-rpm install-apk rpm apk help

all: build

help:
	@echo "PhotoSync System Build Targets:"
	@echo ""
	@echo "  make build          - Build release binaries"
	@echo "  make test           - Run tests"
	@echo "  make install        - Install to $(PREFIX)"
	@echo "  make rpm            - Build RPM package (Fedora/RHEL)"
	@echo "  make apk            - Build APK package (Alpine)"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make help           - Show this help"
	@echo ""
	@echo "Environment variables:"
	@echo "  PREFIX=$(PREFIX)"
	@echo "  DESTDIR=$(DESTDIR)"

build:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --release --locked

clean:
	$(CARGO) clean
	rm -rf target/
	rm -rf packaging/rpm/RPMS/
	rm -rf packaging/rpm/SRPMS/
	rm -rf packaging/apk/*.apk

install: build
	# Install binaries
	$(INSTALL) -Dm755 target/release/photosync-daemon $(DESTDIR)$(BIN_DIR)/photosync-daemon
	$(INSTALL) -Dm755 target/release/photosync-cli $(DESTDIR)$(BIN_DIR)/photosync-cli
	$(INSTALL) -Dm755 scripts/setup-syncthing.sh $(DESTDIR)$(BIN_DIR)/photosync-setup
	
	# Install systemd service
	$(INSTALL) -Dm644 systemd/photosync.service $(DESTDIR)$(SYSTEMD_USER_DIR)/photosync.service
	
	# Create state directory
	$(INSTALL) -dm755 $(DESTDIR)$(STATE_DIR)
	
	# Install documentation
	$(INSTALL) -Dm644 README.md $(DESTDIR)$(PREFIX)/share/doc/photosync-system/README.md

rpm: build
	@echo "Building RPM package..."
	@mkdir -p ~/rpmbuild/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
	@cp packaging/rpm/photosync-system.spec ~/rpmbuild/SPECS/
	@tar czf ~/rpmbuild/SOURCES/photosync-system-0.1.0.tar.gz \
		--transform 's,^,photosync-system-0.1.0/,' \
		Cargo.toml src/ systemd/ scripts/ packaging/ README.md
	rpmbuild -bb ~/rpmbuild/SPECS/photosync-system.spec
	@echo ""
	@echo "RPM package built:"
	@ls -lh ~/rpmbuild/RPMS/*/*.rpm
	@echo ""
	@echo "Install with: sudo dnf install ~/rpmbuild/RPMS/*/*.rpm"

apk:
	@echo "Building APK package..."
	@if [ ! -f /etc/alpine-release ]; then \
		echo "ERROR: APK packages can only be built on Alpine Linux"; \
		exit 1; \
	fi
	@mkdir -p ~/packages/photosync-system
	@cp -r packaging/apk/APKBUILD ~/packages/photosync-system/
	@tar czf ~/packages/photosync-system/photosync-system-0.1.0.tar.gz \
		--transform 's,^,photosync-system-0.1.0/,' \
		Cargo.toml src/ systemd/ scripts/ packaging/ README.md
	cd ~/packages/photosync-system && abuild -r
	@echo ""
	@echo "APK package built:"
	@ls -lh ~/packages/photosync-system/*.apk
	@echo ""
	@echo "Install with: sudo apk add ~/packages/photosync-system/*.apk"

uninstall:
	rm -f $(DESTDIR)$(BIN_DIR)/photosync-daemon
	rm -f $(DESTDIR)$(BIN_DIR)/photosync-cli
	rm -f $(DESTDIR)$(BIN_DIR)/photosync-setup
	rm -f $(DESTDIR)$(SYSTEMD_USER_DIR)/photosync.service
	rm -rf $(DESTDIR)$(PREFIX)/share/doc/photosync-system

.PHONY: dev-setup
dev-setup:
	@echo "Setting up development environment..."
	@echo "Installing Rust dependencies..."
	$(CARGO) fetch
	@echo ""
	@echo "Development setup complete!"
	@echo "Run 'make build' to build the project"
	@echo "Run 'make test' to run tests"
