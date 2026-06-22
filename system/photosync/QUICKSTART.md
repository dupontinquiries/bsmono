# PhotoSync Quick Start Guide

Get up and running with PhotoSync in 5 minutes!

## 1. Install PhotoSync

### Fedora / RHEL / CentOS

```bash
# Install the package
sudo dnf install photosync-system-0.1.0-1.*.rpm

# Or build from source
git clone https://github.com/photosync/photosync-system
cd photosync-system
make build
sudo make install
```

### Alpine Linux

```bash
# Install the package
sudo apk add photosync-system-0.1.0-r0.apk

# Or build from source
git clone https://github.com/photosync/photosync-system
cd photosync-system
make build
sudo make install
```

## 2. Initial Setup

Run the setup wizard:

```bash
photosync-setup
```

This will:
- Create necessary directories
- Set up Syncthing configuration
- Create systemd/OpenRC services
- Generate your device ID

## 3. Start the Service

### On systemd (most Linux distros)

```bash
systemctl --user enable photosync.service
systemctl --user start photosync.service
systemctl --user status photosync.service
```

### On OpenRC (Alpine Linux)

```bash
sudo rc-update add photosync default
sudo rc-service photosync start
sudo rc-service photosync status
```

## 4. Configure Syncthing

1. Open Syncthing web interface:
   ```bash
   xdg-open http://localhost:8384
   ```

2. Add your photo folder:
   - Click "Add Folder"
   - Folder Path: `/home/youruser/Pictures/PhotoSync`
   - Folder ID: `photosync-photos`
   - Enable "File Versioning" → Simple → Keep 5 versions

3. Share with other devices:
   - Click "Add Remote Device"
   - Enter the Device ID from your other device
   - Select the photosync-photos folder to share

## 5. Add Photos

Simply copy photos to the sync directory:

```bash
# From camera
cp /media/camera/DCIM/*.jpg ~/Pictures/PhotoSync/

# From downloads
cp ~/Downloads/*.jpg ~/Pictures/PhotoSync/

# Or use your file manager
```

PhotoSync will automatically:
- ✅ Compress files (zstd level 19)
- ✅ Deduplicate blocks
- ✅ Generate thumbnails
- ✅ Sync to other devices

## 6. Verify Everything Works

Check the statistics:

```bash
photosync-cli stats
```

You should see output like:

```
PhotoSync Storage Statistics
============================
Total original size:    1.23 GB
Compressed size:        650.45 MB
Space saved:            596.55 MB
Compression ratio:      52.9%
Number of blocks:       1247
```

## Common Setup Scenarios

### Laptop + Phone Sync

**On Laptop:**
```bash
# Install and setup
sudo dnf install photosync-system  # or: sudo apk add photosync-system
photosync-setup
systemctl --user enable --now photosync.service

# Get Device ID
syncthing --device-id
# Example output: ABCD1234-EFGH5678-IJKL9012-MNOP3456-...
```

**On Phone (PinePhone, Librem 5, etc.):**
```bash
# Install
sudo apk add photosync-system

# Setup
photosync-setup
sudo rc-update add photosync default
sudo rc-service photosync start

# Get Device ID
syncthing --device-id
```

**Connect Devices:**
1. On laptop: Open http://localhost:8384
2. Click "Add Remote Device"
3. Paste phone's Device ID
4. Select folders to share
5. Repeat on phone's Syncthing

### Multiple Family Members

Each family member sets up PhotoSync on their device, then:

1. **Share Device IDs securely** (Signal, in-person, etc.)
2. **Add devices in Syncthing** on each person's machine
3. **Create shared folders:**
   ```bash
   mkdir ~/Pictures/PhotoSync/Family
   mkdir ~/Pictures/PhotoSync/Personal
   ```
4. **Configure folder sharing** in Syncthing (share Family, not Personal)

### Home Server Backup

Set up a central backup server:

```bash
# On server
sudo dnf install photosync-system syncthing
photosync-setup

# Set large storage
sudo mkdir -p /mnt/backup/photosync
sudo ln -s /mnt/backup/photosync /var/lib/photosync/blocks

# Start services
systemctl --user enable --now photosync.service
systemctl --user enable --now photosync-syncthing.service
```

Connect all family devices to the server for automatic backup.

## Troubleshooting

### Service won't start

```bash
# Check logs
journalctl --user -u photosync.service -n 50

# Check permissions
ls -la /var/lib/photosync/
ls -la ~/Pictures/PhotoSync/
```

### Syncthing not connecting

```bash
# Check firewall
sudo firewall-cmd --add-service=syncthing --permanent
sudo firewall-cmd --reload

# Check Syncthing logs
journalctl --user -u photosync-syncthing.service -n 50
```

### Out of disk space

```bash
# Check storage usage
photosync-cli stats

# Run garbage collection
photosync-cli gc

# Remove old versions
# (In Syncthing GUI: Actions → Versions → Clean)
```

### High CPU usage

```bash
# Lower process priority
systemctl --user edit photosync.service

# Add:
[Service]
Nice=19
```

## Next Steps

- **Customize configuration:** Edit `~/.config/photosync/config.toml`
- **Set up encryption:** Use LUKS for block storage
- **Enable cloud backup:** Connect to cloud storage via rclone
- **Monitor performance:** Check `photosync-cli stats` regularly
- **Read full docs:** See README.md for advanced features

## Getting Help

- **Documentation:** `/usr/share/doc/photosync-system/README.md`
- **Man pages:** `man photosync-daemon`, `man photosync-cli`
- **Logs:** `journalctl --user -u photosync.service -f`
- **GitHub Issues:** https://github.com/photosync/photosync-system/issues

## Security Checklist

- ✅ Keep Device IDs private
- ✅ Use strong passwords for Syncthing GUI
- ✅ Enable TLS (automatic in Syncthing)
- ✅ Restrict Syncthing to local network/VPN
- ✅ Enable file versioning
- ✅ Regular backups to external drive
- ✅ Encrypt disk with LUKS (recommended)

Enjoy your automated, compressed, deduplicated photo sync!
