Name:           photosync-system
Version:        0.1.0
Release:        1%{?dist}
Summary:        High-compression photo sync system with deduplication and camera GUI

License:        MIT OR Apache-2.0
URL:            https://github.com/photosync/photosync-system
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.70
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  cmake >= 3.16
BuildRequires:  sqlite-devel
BuildRequires:  zstd-devel
BuildRequires:  systemd-rpm-macros
BuildRequires:  qt5-qtbase-devel
BuildRequires:  qt5-qtdeclarative-devel
BuildRequires:  qt5-qtquickcontrols2-devel
BuildRequires:  qt5-qtmultimedia-devel
BuildRequires:  libcamera-devel
BuildRequires:  mesa-libgbm-devel
BuildRequires:  mesa-libEGL-devel
BuildRequires:  libdrm-devel

Requires:       syncthing
Requires:       zstd
Requires:       sqlite
Requires:       libcamera
Requires:       qt5-qtbase
Requires:       qt5-qtdeclarative
Requires:       qt5-qtquickcontrols2
Requires:       qt5-qtmultimedia
Requires:       mesa-libgbm
Requires:       mesa-libEGL

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
- Snapchat-style camera GUI with libcamera
- Advanced camera controls (focus, white balance, ToF)
- Hardware-accelerated rendering

%package gui
Summary:        PhotoSync camera GUI with libcamera integration
Requires:       %{name} = %{version}-%{release}

%description gui
Snapchat-style camera interface for PhotoSync with aggressive camera
detection, libcamera integration, and professional manual controls.

%prep
%autosetup

%build
# Build Rust components
cargo build --release --locked

# Build Qt GUI
cd gui
mkdir build
cd build
cmake .. \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=%{_prefix}
make %{?_smp_mflags}
cd ../..

%install
# Install binaries
install -Dpm 0755 target/release/photosync-daemon %{buildroot}%{_bindir}/photosync-daemon
install -Dpm 0755 target/release/photosync-cli %{buildroot}%{_bindir}/photosync-cli

# Install GUI
cd gui/build
make DESTDIR=%{buildroot} install
cd ../..

# Install systemd service files
install -Dpm 0644 systemd/photosync.service %{buildroot}%{_userunitdir}/photosync.service

# Install scripts
install -Dpm 0755 scripts/setup-syncthing.sh %{buildroot}%{_bindir}/photosync-setup

# Create state directory
install -dm 0755 %{buildroot}%{_sharedstatedir}/photosync

# Install documentation
install -Dpm 0644 README.md %{buildroot}%{_docdir}/%{name}/README.md
install -Dpm 0644 gui/README.md %{buildroot}%{_docdir}/%{name}/GUI-README.md

%files
%license LICENSE
%doc README.md
%{_bindir}/photosync-daemon
%{_bindir}/photosync-cli
%{_bindir}/photosync-setup
%{_userunitdir}/photosync.service
%dir %attr(0755, root, root) %{_sharedstatedir}/photosync

%files gui
%{_bindir}/photosync-camera
%{_datadir}/applications/photosync-camera.desktop
%{_datadir}/icons/hicolor/scalable/apps/photosync-camera.svg
%doc %{_docdir}/%{name}/GUI-README.md

%post
echo "PhotoSync installed successfully!"
echo ""
echo "To set up PhotoSync:"
echo "  1. Run: photosync-setup"
echo "  2. Enable the service: systemctl --user enable photosync.service"
echo "  3. Start the service: systemctl --user start photosync.service"
echo ""
echo "Camera GUI: photosync-camera"
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
- Snapchat-style camera GUI
- libcamera integration with advanced controls
- Hardware-accelerated rendering
- ToF sensor support
