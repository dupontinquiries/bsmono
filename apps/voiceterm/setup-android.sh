#!/bin/bash
# setup-android.sh — Install Android build dependencies
# Run once to set up the build environment

set -e
echo "=== Web2QML Android Build Setup ==="

# 1. Rust Android targets
echo ""
echo "--- Rust targets ---"
rustup target add aarch64-linux-android    # ARM64 (most modern phones)
rustup target add armv7-linux-androideabi  # ARM32 (older phones)
rustup target add x86_64-linux-android     # x86_64 (emulators)

# 2. cargo-ndk (simplifies NDK cross-compilation)
echo ""
echo "--- cargo-ndk ---"
cargo install cargo-ndk

# 3. Android SDK/NDK
echo ""
echo "--- Android SDK/NDK ---"
if [ -z "$ANDROID_SDK_ROOT" ]; then
    echo "You need to install Android SDK. Options:"
    echo ""
    echo "  Option A: Android Studio (easiest)"
    echo "    Download from https://developer.android.com/studio"
    echo "    Install, then use SDK Manager to install:"
    echo "    - Android SDK Platform 33"  
    echo "    - Android NDK (latest)"
    echo "    - Android SDK Build-Tools"
    echo ""
    echo "  Option B: Command-line tools only (lighter)"
    echo "    wget https://dl.google.com/android/repository/commandlinetools-linux-latest.zip"
    echo "    unzip to ~/Android/cmdline-tools/latest/"
    echo "    sdkmanager 'platform-tools' 'platforms;android-33' 'ndk;25.2.9519653' 'build-tools;33.0.2'"
    echo ""
    echo "  Then add to your shell profile:"
    echo "    export ANDROID_SDK_ROOT=~/Android/Sdk"
    echo "    export ANDROID_NDK_HOME=\$ANDROID_SDK_ROOT/ndk/25.2.9519653"
else
    echo "ANDROID_SDK_ROOT=$ANDROID_SDK_ROOT (OK)"
fi

# 4. Qt6 for Android
echo ""
echo "--- Qt6 for Android ---"
if [ -z "$QT_ANDROID" ]; then
    echo "You need Qt6 for Android. Options:"
    echo ""
    echo "  Option A: Qt Maintenance Tool"
    echo "    If you have Qt installed, run MaintenanceTool"
    echo "    Add: Qt 6.x > Android ARM64"
    echo ""
    echo "  Option B: aqt (command-line installer)"
    echo "    pip install aqtinstall"
    echo "    aqt install-qt linux android 6.6.0 android_arm64_v8a"
    echo ""
    echo "  Then set:"
    echo "    export QT_ANDROID=~/Qt/6.6.0/android_arm64_v8a"
else
    echo "QT_ANDROID=$QT_ANDROID (OK)"
fi

# 5. Summary
echo ""
echo "=== After installing dependencies, add to ~/.bashrc or ~/.zshrc ==="
echo ""
echo "export ANDROID_SDK_ROOT=~/Android/Sdk"  
echo "export ANDROID_NDK_HOME=\$ANDROID_SDK_ROOT/ndk/<version>"
echo "export QT_ANDROID=~/Qt/6.x.x/android_arm64_v8a"
echo ""
echo "Then run: ./build-android.sh"
