Name:           photosync-system
Version:        0.1.0
Release:        1%{?dist}
Summary:        High-compression photo sync system with deduplication

License:        MIT OR Apache-2.0
URL:            https://github.com/photosync/photosync-system
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.70
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  sqlite-devel
BuildRequires:  zstd-devel
BuildRequires:  systemd-rpm-macros

Requires:       syncthing
Requires:       zstd
Requires:       sqlite
Requires:       libcamera

%description
PhotoSync is a powerful photo synchronization system that combines
Syncthing with advanced compression (zstd level 19), block-level
deduplication, and thumbnail caching. It's designed for efficient
photo backup and sync across Linux devices including laptops and phones.

Features:
- 30-50%% compression using zstd
- Block-level deduplication for burst photos
- Automatic thumbnail generation
- SQLite-based metadata and block tracking
- Integration with Syncthing for network sync
- systemd service for automatic processing

%prep
%autosetup

%build
cargo build --release --locked

%install
# Install binaries
install -Dpm 0755 target/release/photosync-daemon %{buildroot}%{_bindir}/photosync-daemon
install -Dpm 0755 target/release/photosync-cli %{buildroot}%{_bindir}/photosync-cli

# Install systemd service files
install -Dpm 0644 systemd/photosync.service %{buildroot}%{_userunitdir}/photosync.service

# Install scripts
install -Dpm 0755 scripts/setup-syncthing.sh %{buildroot}%{_bindir}/photosync-setup

# Create state directory
install -dm 0755 %{buildroot}%{_sharedstatedir}/photosync

# Install documentation
install -Dpm 0644 README.md %{buildroot}%{_docdir}/%{name}/README.md

%files
%license LICENSE
%doc README.md
%{_bindir}/photosync-daemon
%{_bindir}/photosync-cli
%{_bindir}/photosync-setup
%{_userunitdir}/photosync.service
%dir %attr(0755, root, root) %{_sharedstatedir}/photosync

%post
echo "PhotoSync installed successfully!"
echo ""
echo "To set up PhotoSync:"
echo "  1. Run: photosync-setup"
echo "  2. Enable the service: systemctl --user enable photosync.service"
echo "  3. Start the service: systemctl --user start photosync.service"
echo ""
echo "For more information, see: %{_docdir}/%{name}/README.md"

%changelog
* Fri Feb 27 2026 PhotoSync Contributors <photosync@example.com> - 0.1.0-1
- Initial release
- Block-level deduplication with SHA-256
- zstd compression at level 19
- Thumbnail generation and caching
- Syncthing integration
- systemd service support
