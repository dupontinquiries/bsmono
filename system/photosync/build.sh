#!/bin/bash
# PhotoSync System - Automated Build Script
# Builds packages for both DNF (Fedora/RHEL) and APK (Alpine)

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

VERSION="0.1.0"
PROJECT_NAME="photosync-system"

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_step() {
    echo -e "${BLUE}[STEP]${NC} $1"
}

show_help() {
    cat << EOF
PhotoSync System Build Script

Usage: $0 [OPTIONS]

OPTIONS:
    --rpm           Build RPM package (Fedora/RHEL/CentOS)
    --apk           Build APK package (Alpine Linux)
    --both          Build both RPM and APK (requires Docker)
    --clean         Clean build artifacts
    --install       Install locally after building
    --help          Show this help message

EXAMPLES:
    $0 --rpm                    # Build RPM on Fedora/RHEL
    $0 --apk                    # Build APK on Alpine
    $0 --both                   # Build both using Docker
    $0 --rpm --install          # Build and install RPM
    $0 --clean                  # Clean all build artifacts

EOF
}

check_dependencies() {
    log_step "Checking dependencies..."
    
    local deps=("cargo" "rustc")
    local missing=()
    
    for dep in "${deps[@]}"; do
        if ! command -v "$dep" &> /dev/null; then
            missing+=("$dep")
        fi
    done
    
    if [ ${#missing[@]} -ne 0 ]; then
        log_error "Missing dependencies: ${missing[*]}"
        log_info "Install with:"
        if [ -f /etc/fedora-release ] || [ -f /etc/redhat-release ]; then
            log_info "  sudo dnf install rust cargo"
        elif [ -f /etc/alpine-release ]; then
            log_info "  sudo apk add rust cargo"
        else
            log_info "  See https://rustup.rs/"
        fi
        exit 1
    fi
    
    log_info "All dependencies satisfied"
}

build_rust() {
    log_step "Building Rust components..."
    cargo build --release --locked
    log_info "Rust build complete"
}

build_rpm() {
    log_step "Building RPM package..."
    
    if ! command -v rpmbuild &> /dev/null; then
        log_error "rpmbuild not found. Install with: sudo dnf install rpm-build"
        exit 1
    fi
    
    # Create rpmbuild directories
    mkdir -p ~/rpmbuild/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
    
    # Copy spec file
    cp packaging/rpm/photosync-system.spec ~/rpmbuild/SPECS/
    
    # Create source tarball
    log_info "Creating source tarball..."
    tar czf ~/rpmbuild/SOURCES/${PROJECT_NAME}-${VERSION}.tar.gz \
        --transform "s,^,${PROJECT_NAME}-${VERSION}/," \
        --exclude='.git' \
        --exclude='target' \
        --exclude='*.rpm' \
        --exclude='*.apk' \
        Cargo.toml Cargo.lock src/ systemd/ scripts/ packaging/ \
        config/ README.md QUICKSTART.md LICENSE Makefile
    
    # Build RPM
    log_info "Building RPM with rpmbuild..."
    rpmbuild -bb ~/rpmbuild/SPECS/photosync-system.spec
    
    # Find and display built RPM
    RPM_PATH=$(find ~/rpmbuild/RPMS -name "${PROJECT_NAME}-*.rpm" -type f | head -n1)
    if [ -n "$RPM_PATH" ]; then
        log_info "RPM package built successfully:"
        ls -lh "$RPM_PATH"
        echo ""
        log_info "Install with:"
        echo "  sudo dnf install $RPM_PATH"
    else
        log_error "RPM build failed - package not found"
        exit 1
    fi
}

build_apk() {
    log_step "Building APK package..."
    
    if [ ! -f /etc/alpine-release ]; then
        log_warn "APK packages should be built on Alpine Linux"
        log_info "Attempting to build anyway..."
    fi
    
    if ! command -v abuild &> /dev/null; then
        log_error "abuild not found. Install with: sudo apk add alpine-sdk"
        exit 1
    fi
    
    # Create package directory
    mkdir -p ~/packages/${PROJECT_NAME}
    
    # Copy APKBUILD and related files
    cp packaging/apk/APKBUILD ~/packages/${PROJECT_NAME}/
    cp packaging/apk/photosync.initd ~/packages/${PROJECT_NAME}/
    cp packaging/apk/photosync.confd ~/packages/${PROJECT_NAME}/
    cp packaging/apk/photosync-system.post-install ~/packages/${PROJECT_NAME}/
    
    # Create source tarball
    log_info "Creating source tarball..."
    tar czf ~/packages/${PROJECT_NAME}/${PROJECT_NAME}-${VERSION}.tar.gz \
        --transform "s,^,${PROJECT_NAME}-${VERSION}/," \
        --exclude='.git' \
        --exclude='target' \
        --exclude='*.rpm' \
        --exclude='*.apk' \
        Cargo.toml Cargo.lock src/ systemd/ scripts/ packaging/ \
        config/ README.md QUICKSTART.md LICENSE Makefile
    
    # Build APK
    log_info "Building APK with abuild..."
    cd ~/packages/${PROJECT_NAME}
    abuild checksum
    abuild -r
    cd -
    
    # Find and display built APK
    APK_PATH=$(find ~/packages -name "${PROJECT_NAME}-*.apk" -type f | head -n1)
    if [ -n "$APK_PATH" ]; then
        log_info "APK package built successfully:"
        ls -lh "$APK_PATH"
        echo ""
        log_info "Install with:"
        echo "  sudo apk add --allow-untrusted $APK_PATH"
    else
        log_error "APK build failed - package not found"
        exit 1
    fi
}

build_both() {
    log_step "Building both RPM and APK using Docker..."
    
    if ! command -v docker &> /dev/null; then
        log_error "Docker not found. Install Docker to build both packages."
        exit 1
    fi
    
    # Build RPM in Fedora container
    log_info "Building RPM in Fedora container..."
    docker run --rm -v "$PWD:/build" -w /build fedora:latest bash -c "
        dnf install -y rust cargo rpm-build sqlite-devel zstd-devel &&
        ./build.sh --rpm
    "
    
    # Build APK in Alpine container
    log_info "Building APK in Alpine container..."
    docker run --rm -v "$PWD:/build" -w /build alpine:latest sh -c "
        apk add --no-cache rust cargo alpine-sdk sqlite-dev zstd-dev sudo &&
        adduser -D builder &&
        echo 'builder ALL=(ALL) NOPASSWD: ALL' >> /etc/sudoers &&
        chown -R builder:builder /build &&
        su builder -c './build.sh --apk'
    "
    
    log_info "Both packages built successfully"
}

clean() {
    log_step "Cleaning build artifacts..."
    
    cargo clean
    rm -rf ~/rpmbuild/BUILD/${PROJECT_NAME}-*
    rm -rf ~/rpmbuild/BUILDROOT/${PROJECT_NAME}-*
    rm -rf ~/rpmbuild/SOURCES/${PROJECT_NAME}-*.tar.gz
    rm -rf ~/packages/${PROJECT_NAME}
    
    log_info "Clean complete"
}

install_local() {
    log_step "Installing locally..."
    
    if [ -f /etc/fedora-release ] || [ -f /etc/redhat-release ]; then
        RPM_PATH=$(find ~/rpmbuild/RPMS -name "${PROJECT_NAME}-*.rpm" -type f | head -n1)
        if [ -n "$RPM_PATH" ]; then
            sudo dnf install -y "$RPM_PATH"
        else
            log_error "No RPM package found. Build first with --rpm"
            exit 1
        fi
    elif [ -f /etc/alpine-release ]; then
        APK_PATH=$(find ~/packages -name "${PROJECT_NAME}-*.apk" -type f | head -n1)
        if [ -n "$APK_PATH" ]; then
            sudo apk add --allow-untrusted "$APK_PATH"
        else
            log_error "No APK package found. Build first with --apk"
            exit 1
        fi
    else
        log_error "Unsupported distribution"
        exit 1
    fi
    
    log_info "Installation complete"
}

# Main script
main() {
    if [ $# -eq 0 ]; then
        show_help
        exit 0
    fi
    
    local action=""
    local install_after=false
    
    while [ $# -gt 0 ]; do
        case "$1" in
            --rpm)
                action="rpm"
                ;;
            --apk)
                action="apk"
                ;;
            --both)
                action="both"
                ;;
            --clean)
                clean
                exit 0
                ;;
            --install)
                install_after=true
                ;;
            --help)
                show_help
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
        shift
    done
    
    # Always check dependencies and build Rust first
    if [ "$action" != "clean" ]; then
        check_dependencies
        build_rust
    fi
    
    case "$action" in
        rpm)
            build_rpm
            ;;
        apk)
            build_apk
            ;;
        both)
            build_both
            ;;
        *)
            log_error "No build target specified"
            show_help
            exit 1
            ;;
    esac
    
    if [ "$install_after" = true ]; then
        install_local
    fi
    
    log_info "Build script complete!"
}

main "$@"
