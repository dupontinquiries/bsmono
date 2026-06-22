#!/bin/bash
# PhotoSync System - Installation Instructions
# 
# This file contains step-by-step instructions for building and installing
# PhotoSync on different Linux distributions.

cat << 'EOF'
╔══════════════════════════════════════════════════════════════════╗
║           PhotoSync System - Installation Guide                  ║
╚══════════════════════════════════════════════════════════════════╝

QUICK START
===========

1. Extract the archive:
   tar xzf photosync-system.tar.gz
   cd photosync-system

2. Choose your build method:
   - For Fedora/RHEL: ./build.sh --rpm --install
   - For Alpine: ./build.sh --apk --install
   - From source: make build && sudo make install

3. Run setup:
   photosync-setup

4. Start using:
   systemctl --user start photosync.service

═══════════════════════════════════════════════════════════════════

DETAILED INSTALLATION
=====================

OPTION 1: Fedora / RHEL / CentOS (DNF)
---------------------------------------

# Install build dependencies
sudo dnf install rust cargo rpm-build sqlite-devel zstd-devel syncthing

# Build RPM package
./build.sh --rpm

# Install the package
sudo dnf install ~/rpmbuild/RPMS/*/photosync-system-*.rpm

# Run setup wizard
photosync-setup

# Enable and start service
systemctl --user enable photosync.service
systemctl --user start photosync.service

# Check status
systemctl --user status photosync.service

═══════════════════════════════════════════════════════════════════

OPTION 2: Alpine Linux (APK)
-----------------------------

# Install build dependencies
sudo apk add rust cargo alpine-sdk sqlite-dev zstd-dev syncthing

# Build APK package
./build.sh --apk

# Install the package
sudo apk add --allow-untrusted ~/packages/photosync-system/*.apk

# Run setup wizard
photosync-setup

# For systemd (if available):
systemctl --user enable photosync.service
systemctl --user start photosync.service

# For OpenRC (Alpine default):
sudo rc-update add photosync default
sudo rc-service photosync start

# Check status
sudo rc-service photosync status

═══════════════════════════════════════════════════════════════════

OPTION 3: Build from Source (Any Linux)
----------------------------------------

# Install dependencies (Fedora example)
sudo dnf install rust cargo sqlite-devel zstd-devel syncthing

# Or for Alpine:
# sudo apk add rust cargo sqlite-dev zstd-dev syncthing

# Build the project
make build

# Install system-wide
sudo make install

# Run setup wizard
photosync-setup

# Enable service
systemctl --user enable photosync.service
systemctl --user start photosync.service

═══════════════════════════════════════════════════════════════════

OPTION 4: Development Build
----------------------------

# For developers who want to test without installing:

# Install dependencies
cargo fetch

# Build in debug mode
cargo build

# Run daemon directly
./target/debug/photosync-daemon \
    --watch-dir ~/Pictures/PhotoSync \
    --db-path /tmp/photosync.db \
    --block-store /tmp/photosync-blocks \
    --device-id "$(hostname)" \
    --thumbnail-size 256 \
    --process-existing

# Run CLI
./target/debug/photosync-cli stats

═══════════════════════════════════════════════════════════════════

POST-INSTALLATION SETUP
=======================

After installing, complete these steps:

1. Configure Syncthing
   -----------------------
   # Open Syncthing web interface
   xdg-open http://localhost:8384
   
   # Add folder:
   - Path: ~/Pictures/PhotoSync
   - Folder ID: photosync-photos
   - File Versioning: Simple (5 versions)
   
   # Share with other devices:
   - Get Device ID: syncthing --device-id
   - Add remote devices in GUI
   - Select folders to share

2. Copy Your Photos
   -----------------------
   # From camera
   cp /media/camera/DCIM/*.jpg ~/Pictures/PhotoSync/
   
   # From downloads
   cp ~/Downloads/*.jpg ~/Pictures/PhotoSync/
   
   # Organize by date (optional)
   mkdir -p ~/Pictures/PhotoSync/$(date +%Y-%m-%d)
   mv ~/Pictures/PhotoSync/*.jpg ~/Pictures/PhotoSync/$(date +%Y-%m-%d)/

3. Verify Operation
   -----------------------
   # Check daemon status
   systemctl --user status photosync.service
   
   # View logs
   journalctl --user -u photosync.service -f
   
   # Check statistics
   photosync-cli stats

4. Configure Advanced Options (Optional)
   -----------------------
   # Copy example config
   mkdir -p ~/.config/photosync
   cp config/config.toml.example ~/.config/photosync/config.toml
   
   # Edit settings
   nano ~/.config/photosync/config.toml
   
   # Restart service
   systemctl --user restart photosync.service

═══════════════════════════════════════════════════════════════════

MULTI-DEVICE SETUP
==================

Laptop + Phone Sync Example:
----------------------------

# On Laptop (Fedora)
sudo dnf install photosync-system
photosync-setup
systemctl --user enable --now photosync.service
syncthing --device-id
# Copy this Device ID

# On Phone (PostmarketOS/Alpine)
sudo apk add photosync-system
photosync-setup
sudo rc-update add photosync default
sudo rc-service photosync start
syncthing --device-id
# Copy this Device ID

# Connect Devices (on both)
# Open http://localhost:8384
# Add Remote Device (paste other device's ID)
# Share ~/Pictures/PhotoSync folder

═══════════════════════════════════════════════════════════════════

TROUBLESHOOTING
===============

Service Won't Start
-------------------
# Check logs
journalctl --user -u photosync.service -n 50

# Verify permissions
ls -la /var/lib/photosync/
ls -la ~/Pictures/PhotoSync/

# Recreate directories
rm -rf /var/lib/photosync/
photosync-setup

Syncthing Not Connecting
-------------------------
# Check firewall
sudo firewall-cmd --list-all
sudo firewall-cmd --add-service=syncthing --permanent
sudo firewall-cmd --reload

# Verify Syncthing is running
systemctl --user status photosync-syncthing.service

# Check ports
ss -tlnp | grep 22000

High CPU Usage
--------------
# Lower priority
systemctl --user edit photosync.service
# Add:
[Service]
Nice=19

# Restart
systemctl --user restart photosync.service

Out of Disk Space
-----------------
# Check usage
photosync-cli stats

# Run garbage collection
photosync-cli gc

# Clean Syncthing versions
# In Syncthing GUI: Actions → Versions → Clean

Database Issues
---------------
# Verify integrity
sqlite3 /var/lib/photosync/photosync.db "PRAGMA integrity_check;"

# Backup current database
cp /var/lib/photosync/photosync.db /var/lib/photosync/photosync.db.backup

# Rebuild (if needed)
photosync-cli verify

═══════════════════════════════════════════════════════════════════

UNINSTALLATION
==============

Fedora/RHEL:
   sudo dnf remove photosync-system

Alpine:
   sudo apk del photosync-system

From Source:
   sudo make uninstall

Clean User Data:
   rm -rf ~/.config/photosync
   rm -rf ~/Pictures/PhotoSync
   rm -rf /var/lib/photosync
   systemctl --user stop photosync.service
   systemctl --user disable photosync.service

═══════════════════════════════════════════════════════════════════

DOCUMENTATION
=============

README.md          - Complete documentation
QUICKSTART.md      - 5-minute setup guide
ARCHITECTURE.md    - Technical design details
INDEX.md           - Project overview
config/config.toml.example - All configuration options

═══════════════════════════════════════════════════════════════════

GETTING HELP
============

GitHub Issues: https://github.com/photosync/photosync-system/issues
Discussions: https://github.com/photosync/photosync-system/discussions
Documentation: /usr/share/doc/photosync-system/

═══════════════════════════════════════════════════════════════════
EOF
