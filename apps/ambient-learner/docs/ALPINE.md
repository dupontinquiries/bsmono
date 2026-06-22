# Building on Alpine Linux

Alpine Linux uses the `apk` package manager and musl libc instead of glibc, which can require some additional setup for Rust GTK4 applications.

## Quick Installation

```bash
# Install Rust, Cargo, and GTK4 development packages
sudo apk add rust cargo gtk4.0-dev

# You may also need these additional packages:
sudo apk add pkgconfig gcc musl-dev
```

## Complete Dependency List

For a full build environment on Alpine:

```bash
sudo apk add \
    rust \
    cargo \
    gtk4.0-dev \
    pkgconfig \
    gcc \
    musl-dev \
    glib-dev \
    cairo-dev \
    pango-dev \
    gdk-pixbuf-dev \
    librsvg-dev
```

## Building the Application

```bash
cd /path/to/ambient-learner
cargo build --release
```

## Alpine-Specific Notes

1. **musl vs glibc**: Alpine uses musl libc by default, which is lighter but may have some compatibility differences. Your application should work fine with musl.

2. **Static linking**: Alpine's Rust packages are configured for static linking by default, which creates larger binaries but makes them more portable.

3. **Package names**: GTK4 packages in Alpine use the `.0-dev` suffix (e.g., `gtk4.0-dev` instead of just `gtk4-dev`).

4. **Edge repository**: If you need the absolute latest GTK4 version, you might need to enable the edge/testing repository:
   ```bash
   sudo apk add gtk4.0-dev --repository=http://dl-cdn.alpinelinux.org/alpine/edge/community
   ```

## Troubleshooting Alpine-Specific Issues

### Issue: "package gtk4 not found"
```bash
# Make sure pkg-config can find GTK4
pkg-config --modversion gtk4

# If not found, install pkgconfig
sudo apk add pkgconfig
```

### Issue: Linking errors
```bash
# Install the complete development toolchain
sudo apk add build-base
```

### Issue: "cannot find -lgcc_s"
This is normal on Alpine with musl. Add this to your `~/.cargo/config.toml`:
```toml
[target.x86_64-unknown-linux-musl]
rustflags = ["-C", "target-feature=-crt-static"]
```

## Running in Docker/Container

If you're building in an Alpine Docker container:

```dockerfile
FROM alpine:latest

RUN apk add --no-cache \
    rust \
    cargo \
    gtk4.0-dev \
    pkgconfig \
    gcc \
    musl-dev

WORKDIR /app
COPY . .
RUN cargo build --release

CMD ["./target/release/ambient-learner"]
```

Note: Running GUI applications in containers requires X11 forwarding or Wayland support to be configured.
