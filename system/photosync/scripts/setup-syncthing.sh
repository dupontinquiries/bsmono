#!/bin/bash
# PhotoSync Syncthing Configuration Helper

set -e

CONFIG_DIR="${HOME}/.config/syncthing"
PHOTOSYNC_DIR="${HOME}/Pictures/PhotoSync"
BLOCK_STORE="/var/lib/photosync/blocks"
DB_PATH="/var/lib/photosync/photosync.db"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if Syncthing is installed
if ! command -v syncthing &> /dev/null; then
    log_error "Syncthing is not installed. Please install it first:"
    echo "  - Fedora/RHEL: sudo dnf install syncthing"
    echo "  - Alpine: sudo apk add syncthing"
    exit 1
fi

# Create necessary directories
log_info "Creating directories..."
mkdir -p "${PHOTOSYNC_DIR}"
mkdir -p "${BLOCK_STORE}"
mkdir -p "$(dirname "${DB_PATH}")"
mkdir -p "${CONFIG_DIR}"

# Generate device ID if Syncthing hasn't been run yet
if [ ! -f "${CONFIG_DIR}/config.xml" ]; then
    log_info "Initializing Syncthing..."
    syncthing --generate="${CONFIG_DIR}" --no-default-folder
fi

# Get device ID
DEVICE_ID=$(syncthing --device-id --home="${CONFIG_DIR}")
log_info "Device ID: ${DEVICE_ID}"

# Create Syncthing ignore patterns for blocks
cat > "${PHOTOSYNC_DIR}/.stignore" << 'EOF'
// PhotoSync Syncthing ignore patterns
// Ignore system files
.DS_Store
Thumbs.db
desktop.ini

// Ignore temp files
*.tmp
*.temp
.~*

// Ignore directories that shouldn't sync
.Trash-*
.cache
EOF

log_info "Created .stignore file"

# Create wrapper script for pre-sync compression
cat > "${HOME}/.local/bin/photosync-presync" << 'EOF'
#!/bin/bash
# Pre-sync hook: Compress new photos before Syncthing picks them up

WATCH_DIR="${HOME}/Pictures/PhotoSync"
TEMP_DIR="${HOME}/Pictures/PhotoSync/.processing"

mkdir -p "${TEMP_DIR}"

# Find new/modified images
find "${WATCH_DIR}" -type f \
    \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.png" -o -iname "*.raw" \) \
    -mmin -5 \
    -not -path "*/.processing/*" | while read -r file; do
    
    echo "Pre-processing: ${file}"
    
    # Move to temp, compress, move back
    # This allows the daemon to pick it up
    basename=$(basename "${file}")
    mv "${file}" "${TEMP_DIR}/${basename}"
    
    # The daemon will process it and move it back
done
EOF

chmod +x "${HOME}/.local/bin/photosync-presync"
log_info "Created pre-sync wrapper script"

# Create systemd user service for Syncthing
mkdir -p "${HOME}/.config/systemd/user"

cat > "${HOME}/.config/systemd/user/photosync-syncthing.service" << EOF
[Unit]
Description=PhotoSync Syncthing Service
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/syncthing serve --no-browser --no-restart --home="${CONFIG_DIR}"
Restart=on-failure
RestartSec=10

# Run pre-sync script every 5 minutes
ExecStartPost=/bin/bash -c 'while true; do sleep 300; ${HOME}/.local/bin/photosync-presync; done &'

[Install]
WantedBy=default.target
EOF

log_info "Created Syncthing systemd service"

# Create configuration template
cat > "${CONFIG_DIR}/photosync-config.template" << 'EOF'
PhotoSync Configuration Template
================================

1. Add this folder in Syncthing GUI (http://localhost:8384):
   - Folder ID: photosync-photos
   - Folder Path: ~/Pictures/PhotoSync
   - File Versioning: Simple File Versioning (Keep 5 versions)
   - Ignore Patterns: Use .stignore file
   
2. Sync settings:
   - Scanning: Every 60 seconds
   - File Pull Order: Alphabetic
   - Sync Protocol: Compress always
   
3. Advanced:
   - Max Conflicts: 10
   - Disable fsync: No (keep enabled for data integrity)
   
4. Share this folder with other devices using their Device IDs

5. On each device, ensure PhotoSync daemon is running:
   systemctl --user enable photosync.service
   systemctl --user start photosync.service
EOF

log_info "Configuration template created at ${CONFIG_DIR}/photosync-config.template"

# Enable services
log_info "Enabling systemd services..."
systemctl --user daemon-reload
systemctl --user enable photosync-syncthing.service
systemctl --user enable photosync.service

log_info "Starting services..."
systemctl --user start photosync-syncthing.service
systemctl --user start photosync.service

log_info ""
log_info "PhotoSync setup complete!"
log_info ""
log_info "Next steps:"
log_info "1. Open Syncthing GUI: http://localhost:8384"
log_info "2. Add ~/Pictures/PhotoSync as a shared folder"
log_info "3. Connect to other devices using device IDs"
log_info "4. Copy photos to ~/Pictures/PhotoSync"
log_info ""
log_info "View status:"
log_info "  systemctl --user status photosync.service"
log_info "  systemctl --user status photosync-syncthing.service"
log_info ""
log_info "View statistics:"
log_info "  photosync-cli stats"
