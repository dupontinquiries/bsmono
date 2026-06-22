#!/bin/bash
# build-android.sh — Build Web2QML for Android
# Prerequisites:
#   - Android NDK (r25+) 
#   - Android SDK with platform 33+
#   - Qt6 for Android (install via Qt Maintenance Tool or aqt)
#   - Rust Android targets: rustup target add aarch64-linux-android armv7-linux-androideabi
#   - cargo-ndk: cargo install cargo-ndk
#
# Environment variables needed:
#   ANDROID_NDK_HOME=/path/to/ndk
#   ANDROID_SDK_ROOT=/path/to/sdk
#   QT_ANDROID=/path/to/Qt/6.x.x/android_arm64_v8a
#
# Usage: ./build-android.sh [debug|release]

set -e

MODE="${1:-release}"
APP_NAME="web2qml"
PKG_NAME="org.web2qml.browser"
MIN_SDK=26
TARGET_SDK=33
ABI="arm64-v8a"
RUST_TARGET="aarch64-linux-android"

echo "=== Web2QML Android Build ==="
echo "Mode: $MODE"
echo "ABI: $ABI ($RUST_TARGET)"

# Check prerequisites
check_env() {
    if [ -z "$ANDROID_NDK_HOME" ]; then echo "ERROR: ANDROID_NDK_HOME not set"; exit 1; fi
    if [ -z "$ANDROID_SDK_ROOT" ]; then echo "ERROR: ANDROID_SDK_ROOT not set"; exit 1; fi
    if [ -z "$QT_ANDROID" ]; then 
        # Try to find Qt Android automatically
        QT_ANDROID=$(find ~/Qt /opt/Qt -name "android_arm64_v8a" -type d 2>/dev/null | head -1)
        if [ -z "$QT_ANDROID" ]; then echo "ERROR: QT_ANDROID not set and Qt Android not found"; exit 1; fi
        echo "Found Qt Android: $QT_ANDROID"
    fi
    
    # Check Rust target
    if ! rustup target list --installed | grep -q "$RUST_TARGET"; then
        echo "Adding Rust target $RUST_TARGET..."
        rustup target add $RUST_TARGET
    fi
    
    # Check cargo-ndk
    if ! command -v cargo-ndk &> /dev/null; then
        echo "Installing cargo-ndk..."
        cargo install cargo-ndk
    fi
}

# Step 1: Build Rust library for Android
build_rust() {
    echo ""
    echo "=== Building Rust for $RUST_TARGET ==="
    
    # Build as a shared library instead of binary
    # We need to add cdylib to Cargo.toml temporarily
    if ! grep -q "cdylib" Cargo.toml; then
        sed -i '/^\[package\]/a\\n[lib]\ncrate-type = ["cdylib"]\nname = "web2qml"' Cargo.toml
        echo "Added cdylib target to Cargo.toml"
    fi
    
    if [ "$MODE" = "release" ]; then
        cargo ndk -t $ABI build --release
        LIB_PATH="target/$RUST_TARGET/release/libweb2qml.so"
    else
        cargo ndk -t $ABI build
        LIB_PATH="target/$RUST_TARGET/debug/libweb2qml.so"
    fi
    
    echo "Built: $LIB_PATH ($(du -h $LIB_PATH | cut -f1))"
}

# Step 2: Create Android project structure
create_android_project() {
    echo ""
    echo "=== Creating Android project ==="
    
    ANDROID_DIR="android-build"
    mkdir -p $ANDROID_DIR/src/main/java/org/web2qml/browser
    mkdir -p $ANDROID_DIR/src/main/res/values
    mkdir -p $ANDROID_DIR/src/main/res/drawable
    mkdir -p $ANDROID_DIR/src/main/assets/qml
    mkdir -p $ANDROID_DIR/libs/$ABI
    
    # Copy QML files
    cp qml/main.qml $ANDROID_DIR/src/main/assets/qml/
    
    # Copy built library
    cp $LIB_PATH $ANDROID_DIR/libs/$ABI/
    
    # AndroidManifest.xml
    cat > $ANDROID_DIR/src/main/AndroidManifest.xml << 'MANIFEST'
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="org.web2qml.browser"
    android:versionCode="1"
    android:versionName="0.1">

    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
    
    <application
        android:label="Web2QML"
        android:icon="@drawable/ic_launcher"
        android:theme="@style/AppTheme"
        android:hardwareAccelerated="true">
        
        <activity
            android:name="org.qtproject.qt.android.bindings.QtActivity"
            android:configChanges="orientation|screenSize|screenLayout|uiMode|locale|fontScale|density"
            android:screenOrientation="unspecified"
            android:exported="true"
            android:launchMode="singleTop">
            
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
            
            <intent-filter>
                <action android:name="android.intent.action.VIEW" />
                <category android:name="android.intent.category.DEFAULT" />
                <category android:name="android.intent.category.BROWSABLE" />
                <data android:scheme="http" />
                <data android:scheme="https" />
            </intent-filter>
            
            <meta-data android:name="android.app.lib_name" android:value="web2qml" />
        </activity>
    </application>
</manifest>
MANIFEST

    # Theme
    cat > $ANDROID_DIR/src/main/res/values/styles.xml << 'STYLES'
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <style name="AppTheme" parent="@android:style/Theme.DeviceDefault.NoActionBar">
        <item name="android:windowBackground">@android:color/black</item>
        <item name="android:statusBarColor">#1e1e1e</item>
        <item name="android:navigationBarColor">#2b2b2b</item>
    </style>
</resources>
STYLES

    # Simple icon (placeholder)
    cat > $ANDROID_DIR/src/main/res/values/strings.xml << 'STRINGS'
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <string name="app_name">Web2QML</string>
</resources>
STRINGS

    echo "Android project created in $ANDROID_DIR/"
}

# Step 3: Build APK using androiddeployqt
build_apk() {
    echo ""
    echo "=== Building APK ==="
    
    # Create deployment settings
    cat > $ANDROID_DIR/android-deployment-settings.json << DEPLOY
{
    "qt": "$QT_ANDROID",
    "sdk": "$ANDROID_SDK_ROOT",
    "ndk": "$ANDROID_NDK_HOME",
    "toolchain-prefix": "llvm",
    "tool-prefix": "llvm",
    "ndk-host": "linux-x86_64",
    "target-architecture": "$ABI",
    "qml-root-path": "$PWD/qml",
    "application-binary": "$PWD/$LIB_PATH",
    "android-package": "$PKG_NAME",
    "android-min-sdk-version": "$MIN_SDK",
    "android-target-sdk-version": "$TARGET_SDK"
}
DEPLOY

    # Run androiddeployqt
    $QT_ANDROID/bin/androiddeployqt \
        --input $ANDROID_DIR/android-deployment-settings.json \
        --output $ANDROID_DIR \
        --android-platform android-$TARGET_SDK \
        --gradle \
        $([ "$MODE" = "release" ] && echo "--release")
    
    # Find the APK
    APK=$(find $ANDROID_DIR -name "*.apk" | head -1)
    if [ -n "$APK" ]; then
        cp "$APK" web2qml.apk
        echo ""
        echo "=== APK built: web2qml.apk ($(du -h web2qml.apk | cut -f1)) ==="
    else
        echo "APK not found — check gradle output"
    fi
}

# Main
check_env
build_rust
create_android_project
build_apk

echo ""
echo "=== Done ==="
echo "Install: adb install web2qml.apk"
echo "Or transfer web2qml.apk to your Android device"
