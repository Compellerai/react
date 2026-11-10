#!/bin/bash
# build.sh - Build, sign, notarize, and create DMG for REACT by Compeller.ai
#
# Usage:
#   ./build.sh              # Full build with signing and notarization
#   ./build.sh --dev        # Build with ad-hoc signing (no notarization)
#
# --dev needs no private settings at all.
#
# A full build needs .env.local (untracked; see .env.example) with:
#   APPLE_ID, APPLE_TEAM_ID, APPLE_PASSWORD   notarization credentials
#   APPLE_SIGNING_IDENTITY                    "Developer ID Application: ..." identity

set -e

cd "$(dirname "$0")"
SCRIPT_DIR="$(pwd)"

# PATH (cargo under sudo, Homebrew), pkg-config, NDI_SDK_DIR, .env.local,
# path-remapping rustflags
source "$SCRIPT_DIR/scripts/env.sh"

echo "=== REACT by Compeller.ai - Build ==="
echo ""

# Parse arguments
DEV_MODE=false
for arg in "$@"; do
    case $arg in
        --dev)
            DEV_MODE=true
            ;;
        --no-license-key)
            # Accepted for old scripts. License blobs are verified with a public
            # key that lives in the source, so there is no key to leave out.
            ;;
        --help|-h)
            echo "Usage: ./build.sh [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --dev             Build with ad-hoc signing (skip notarization)"
            echo "  --help            Show this help message"
            exit 0
            ;;
    esac
done

# Signing identity and notarization credentials come from .env.local (loaded by
# scripts/env.sh) — they are never stored in this repository.
if [ "$DEV_MODE" = false ]; then
    if [ -z "$APPLE_ID" ] || [ -z "$APPLE_TEAM_ID" ] || [ -z "$APPLE_PASSWORD" ] || [ -z "$APPLE_SIGNING_IDENTITY" ]; then
        echo "ERROR: Apple signing/notarization env vars not set."
        echo "Create $SCRIPT_DIR/.env.local (see .env.example) with:"
        echo '  export APPLE_ID="you@example.com"'
        echo '  export APPLE_TEAM_ID="YOURTEAMID"'
        echo '  export APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx"'
        echo '  export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Company (YOURTEAMID)"'
        echo ""
        echo "Or run with --dev for ad-hoc signing (no notarization)"
        exit 1
    fi
fi

# Set deployment target
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-12.0}

# Step 1: Setup NDI SDK and download models
echo "[1/5] Setting up NDI SDK and models..."

# Download YOLOv8n ONNX model if not present
MODELS_DIR="$SCRIPT_DIR/models"
YOLO_MODEL="$MODELS_DIR/yolov8n.onnx"
YOLO_URL="https://github.com/ultralytics/assets/releases/download/v8.2.0/yolov8n.onnx"

mkdir -p "$MODELS_DIR"
if [ ! -f "$YOLO_MODEL" ]; then
    echo "       Downloading YOLOv8n ONNX model..."
    curl -L -o "$YOLO_MODEL" "$YOLO_URL"
    if [ -f "$YOLO_MODEL" ]; then
        echo "       Downloaded yolov8n.onnx ($(du -h "$YOLO_MODEL" | cut -f1))"
    else
        echo "       WARNING: Failed to download YOLOv8n model"
    fi
else
    echo "       YOLOv8n model already present"
fi

if [ -d "$NDI_SDK_DIR" ]; then
    echo "       NDI SDK: $NDI_SDK_DIR"
elif [ "$DEV_MODE" = true ]; then
    echo "       WARNING: NDI SDK not found at $NDI_SDK_DIR — building without NDI output"
else
    echo "ERROR: NDI SDK not found at $NDI_SDK_DIR"
    echo "Install the NDI SDK there, or set NDI_SDK_DIR (see .env.example)"
    exit 1
fi

# Step 2: Clean previous builds
echo "[2/5] Cleaning previous builds..."
rm -rf "$SCRIPT_DIR/target/release/bundle"

# Step 3: Build release binary
echo "[3/5] Building release binary..."
cargo build --release --features camera

# Step 4: Create app bundle with cargo-bundle
echo "[4/5] Creating macOS app bundle..."

# Install cargo-bundle if needed
if ! command -v cargo-bundle &> /dev/null; then
    echo "       Installing cargo-bundle..."
    cargo install cargo-bundle
fi

cargo bundle --release --features camera

# Find the app bundle
APP_BUNDLE=$(ls -d "$SCRIPT_DIR/target/release/bundle/osx/"*.app 2>/dev/null | head -n 1)
if [ -z "$APP_BUNDLE" ]; then
    APP_BUNDLE=$(ls -d "$SCRIPT_DIR/target/release/bundle/macos/"*.app 2>/dev/null | head -n 1)
fi

if [ -z "$APP_BUNDLE" ] || [ ! -d "$APP_BUNDLE" ]; then
    echo "ERROR: App bundle not found"
    exit 1
fi

echo "       App bundle: $APP_BUNDLE"

# Add required Info.plist keys for camera/microphone permissions
INFO_PLIST="$APP_BUNDLE/Contents/Info.plist"
echo "       Adding privacy usage descriptions to Info.plist..."

# Camera usage (for USB webcam tracking)
/usr/libexec/PlistBuddy -c "Add :NSCameraUsageDescription string 'REACT uses your camera for real-time subject tracking and audio-reactive visual effects.'" "$INFO_PLIST" 2>/dev/null || \
/usr/libexec/PlistBuddy -c "Set :NSCameraUsageDescription 'REACT uses your camera for real-time subject tracking and audio-reactive visual effects.'" "$INFO_PLIST"

# Microphone usage (for audio input)
/usr/libexec/PlistBuddy -c "Add :NSMicrophoneUsageDescription string 'REACT uses your microphone to capture audio for real-time visualization.'" "$INFO_PLIST" 2>/dev/null || \
/usr/libexec/PlistBuddy -c "Set :NSMicrophoneUsageDescription 'REACT uses your microphone to capture audio for real-time visualization.'" "$INFO_PLIST"

# Copy resources into app bundle
RESOURCES_DIR="$APP_BUNDLE/Contents/Resources"
mkdir -p "$RESOURCES_DIR"

# Copy presets for projectM
echo "       Copying presets..."
if [ -d "$SCRIPT_DIR/presets" ]; then
    cp -r "$SCRIPT_DIR/presets" "$RESOURCES_DIR/"
    PRESET_COUNT=$(find "$RESOURCES_DIR/presets" -name "*.milk" 2>/dev/null | wc -l | tr -d ' ')
    echo "       Copied $PRESET_COUNT presets to Resources"
else
    echo "       WARNING: Presets directory not found at $SCRIPT_DIR/presets"
fi

# Copy ONNX models for camera tracking
echo "       Copying models..."
if [ -d "$SCRIPT_DIR/models" ]; then
    mkdir -p "$RESOURCES_DIR/models"
    # Copy .onnx files only (not README)
    find "$SCRIPT_DIR/models" -name "*.onnx" -exec cp {} "$RESOURCES_DIR/models/" \;
    MODEL_COUNT=$(find "$RESOURCES_DIR/models" -name "*.onnx" 2>/dev/null | wc -l | tr -d ' ')
    if [ "$MODEL_COUNT" -gt 0 ]; then
        echo "       Copied $MODEL_COUNT ONNX models to Resources"
    else
        echo "       WARNING: No .onnx models found in $SCRIPT_DIR/models"
    fi
fi

# Copy LLM instruct models
echo "       Copying LLM instruct models..."
LLM_INSTRUCT_DIR="$SCRIPT_DIR/llm/instruct"
if [ -d "$LLM_INSTRUCT_DIR" ]; then
    GGUF_COUNT=$(find "$LLM_INSTRUCT_DIR" -name "*.gguf" 2>/dev/null | wc -l | tr -d ' ')
    if [ "$GGUF_COUNT" -gt 0 ]; then
        mkdir -p "$RESOURCES_DIR/llm/instruct"
        find "$LLM_INSTRUCT_DIR" -name "*.gguf" -exec cp {} "$RESOURCES_DIR/llm/instruct/" \;
        echo "       Copied $GGUF_COUNT GGUF model(s) to Resources/llm/instruct"
    else
        echo "       WARNING: No .gguf models found in $LLM_INSTRUCT_DIR"
    fi
else
    echo "       WARNING: LLM instruct directory not found at $LLM_INSTRUCT_DIR"
fi

# Copy public folder (fonts, images, videos, compeller-girl)
echo "       Copying public assets..."
if [ -d "$SCRIPT_DIR/public" ]; then
    cp -r "$SCRIPT_DIR/public" "$RESOURCES_DIR/"
    echo "       Copied public/ to Resources"
else
    echo "       WARNING: public directory not found at $SCRIPT_DIR/public"
fi

# Ship the license, attribution and third-party notices with the app
cp "$SCRIPT_DIR/LICENSE" "$SCRIPT_DIR/NOTICE" "$SCRIPT_DIR/THIRD-PARTY-NOTICES.md" "$RESOURCES_DIR/"
echo "       Copied LICENSE, NOTICE and third-party notices to Resources"

# Copy NDI library into app bundle
echo "       Copying NDI library..."
FRAMEWORKS_DIR="$APP_BUNDLE/Contents/Frameworks"
mkdir -p "$FRAMEWORKS_DIR"

NDI_LIB="$NDI_SDK_DIR/lib/macOS/libndi.dylib"
if [ ! -f "$NDI_LIB" ]; then
    NDI_LIB="$NDI_SDK_DIR/lib/libndi.dylib"
fi

if [ -f "$NDI_LIB" ]; then
    cp "$NDI_LIB" "$FRAMEWORKS_DIR/"
    echo "       Copied libndi.dylib"

    # Update rpath in binary
    BINARY="$APP_BUNDLE/Contents/MacOS/compeller-react"
    if [ -f "$BINARY" ]; then
        install_name_tool -add_rpath "@executable_path/../Frameworks" "$BINARY" 2>/dev/null || true
    fi
else
    echo "       WARNING: NDI library not found at $NDI_LIB"
fi

# Copy embedded ffmpeg with NDI support (if available)
echo "       Checking for embedded ffmpeg..."
FFMPEG_SRC="$SCRIPT_DIR/ffmpeg/ffmpeg"

if [ -f "$FFMPEG_SRC" ]; then
    echo "       Copying embedded ffmpeg with NDI support..."
    cp "$FFMPEG_SRC" "$RESOURCES_DIR/"
    chmod +x "$RESOURCES_DIR/ffmpeg"

    # Copy ffmpeg libraries if present
    if [ -d "$SCRIPT_DIR/ffmpeg/lib" ]; then
        echo "       Copying ffmpeg libraries..."
        cp -r "$SCRIPT_DIR/ffmpeg/lib/"*.dylib "$FRAMEWORKS_DIR/" 2>/dev/null || true
    fi

    # Copy NDI runtime for ffmpeg if present
    if [ -f "$SCRIPT_DIR/ffmpeg/libndi.dylib" ]; then
        cp "$SCRIPT_DIR/ffmpeg/libndi.dylib" "$FRAMEWORKS_DIR/"
    fi

    echo "       Embedded ffmpeg ready (NDI streaming enabled)"
else
    echo "       No embedded ffmpeg found at $FFMPEG_SRC"
    echo "       Streaming will use system ffmpeg (if available)"
    echo ""
    echo "       To enable NDI input for ffmpeg, build a custom ffmpeg:"
    echo "         1. Download NDI SDK from ndi.video"
    echo "         2. Build ffmpeg with: --enable-libndi_newtek --enable-nonfree"
    echo "         3. Place binary at: $SCRIPT_DIR/ffmpeg/ffmpeg"
fi

# Bundle Homebrew dylibs so the app runs on machines without Homebrew
echo "       Bundling Homebrew dylibs..."
BINARY="$APP_BUNDLE/Contents/MacOS/compeller-react"
VISITED_DEPS=$(mktemp /tmp/react_deps_visited.XXXXXX)
COLLECTED_DEPS=$(mktemp /tmp/react_deps_collected.XXXXXX)

# Recursive function: walk otool -L output and collect all Homebrew dylib paths
collect_homebrew_deps() {
    local target="$1"
    local resolved
    resolved=$(realpath "$target" 2>/dev/null || echo "$target")

    # Skip if already visited
    if grep -qxF "$resolved" "$VISITED_DEPS" 2>/dev/null; then
        return
    fi
    echo "$resolved" >> "$VISITED_DEPS"

    # Find all /opt/homebrew deps of this target
    otool -L "$resolved" 2>/dev/null | awk '{print $1}' | grep '^/opt/homebrew/' | while read -r dep; do
        local dep_resolved
        dep_resolved=$(realpath "$dep" 2>/dev/null || echo "$dep")
        if [ -f "$dep_resolved" ]; then
            # Record: original_path<TAB>resolved_path
            echo "$dep	$dep_resolved" >> "$COLLECTED_DEPS"
            # Recurse into this dylib
            collect_homebrew_deps "$dep_resolved"
        fi
    done
}

# Walk from the main binary
collect_homebrew_deps "$BINARY"

# Copy phase: deduplicate by resolved path and copy to Frameworks/
echo "       Copying discovered dylibs to Frameworks/..."
DYLIB_COUNT=0
while IFS=$'\t' read -r _orig resolved; do
    fname=$(basename "$resolved")
    if [ ! -f "$FRAMEWORKS_DIR/$fname" ]; then
        cp "$resolved" "$FRAMEWORKS_DIR/$fname"
        chmod 644 "$FRAMEWORKS_DIR/$fname"
        DYLIB_COUNT=$((DYLIB_COUNT + 1))
    fi
done < <(sort -t$'\t' -k2 -u "$COLLECTED_DEPS")

# Copy projectM dylibs
for pm_lib in "$SCRIPT_DIR/libs/libprojectM-4.4.dylib" "$SCRIPT_DIR/libs/libprojectM-4-playlist.4.dylib"; do
    if [ -f "$pm_lib" ]; then
        fname=$(basename "$pm_lib")
        cp "$pm_lib" "$FRAMEWORKS_DIR/$fname"
        chmod 644 "$FRAMEWORKS_DIR/$fname"
        DYLIB_COUNT=$((DYLIB_COUNT + 1))
    fi
done

echo "       Copied $DYLIB_COUNT dylibs to Frameworks/"

# Clean up temp files (no longer needed)
rm -f "$VISITED_DEPS" "$COLLECTED_DEPS"

# Rewrite phase: fix all Homebrew paths in the binary and in each dylib

# Helper: rewrite Homebrew refs in a single Mach-O file
rewrite_homebrew_refs() {
    local target="$1"
    local ref_prefix="$2"  # @executable_path/../Frameworks or @loader_path

    otool -L "$target" 2>/dev/null | awk '{print $1}' | grep '^/opt/homebrew/' | while read -r old_path; do
        local old_resolved
        old_resolved=$(realpath "$old_path" 2>/dev/null || echo "$old_path")
        local lib_name
        lib_name=$(basename "$old_resolved")
        install_name_tool -change "$old_path" "$ref_prefix/$lib_name" "$target" 2>/dev/null || true
    done
}

echo "       Rewriting binary dylib paths..."
rewrite_homebrew_refs "$BINARY" "@executable_path/../Frameworks"

echo "       Rewriting dylib-to-dylib paths..."
for dylib in "$FRAMEWORKS_DIR"/*.dylib; do
    [ -f "$dylib" ] || continue
    fname=$(basename "$dylib")

    # Fix the dylib's own install name
    install_name_tool -id "@loader_path/$fname" "$dylib" 2>/dev/null || true

    # Rewrite its Homebrew refs to @loader_path/
    rewrite_homebrew_refs "$dylib" "@loader_path"
done

# Verify phase: fail build if any /opt/homebrew paths remain
echo "       Verifying no Homebrew paths remain..."
LEAK_FOUND=false
while IFS= read -r -d '' f; do
    if file "$f" | grep -q "Mach-O"; then
        LEAKS=$(otool -L "$f" 2>/dev/null | awk '{print $1}' | grep '^/opt/homebrew/' || true)
        if [ -n "$LEAKS" ]; then
            echo "       ERROR: $(basename "$f") still references Homebrew:"
            echo "$LEAKS" | sed 's/^/         /'
            LEAK_FOUND=true
        fi
    fi
done < <(find "$APP_BUNDLE/Contents" -type f -print0)

if [ "$LEAK_FOUND" = true ]; then
    echo "ERROR: Homebrew dylib bundling incomplete - aborting build"
    exit 1
fi
echo "       All Homebrew paths successfully rewritten"

# Bundle GStreamer plugins (loaded at runtime, not linked - otool won't find them)
echo "       Bundling GStreamer plugins..."
GST_PLUGIN_SRC="/opt/homebrew/lib/gstreamer-1.0"
GST_PLUGIN_DST="$RESOURCES_DIR/gstreamer-1.0"

if [ -d "$GST_PLUGIN_SRC" ]; then
    mkdir -p "$GST_PLUGIN_DST"

    # Required plugins for streaming and audio capture
    GST_REQUIRED_PLUGINS=(
        libgstcoreelements     # queue, capsfilter, tee
        libgstapp              # appsrc, appsink
        libgstaudioconvert     # audioconvert
        libgstaudioresample    # audioresample
        libgstvideoconvertscale # videoconvert, videoscale
        libgstvideofilter      # videoflip
        libgstvideorate        # videorate
        libgstapplemedia       # vtenc_h264 (VideoToolbox)
        libgstvideoparsersbad  # h264parse
        libgstflv              # flvmux
        libgstrtmp             # rtmpsink
        libgstfdkaac           # fdkaacenc
        libgstosxaudio         # osxaudiosrc
    )

    GST_COUNT=0
    for plugin_name in "${GST_REQUIRED_PLUGINS[@]}"; do
        plugin_file=$(ls "$GST_PLUGIN_SRC/${plugin_name}".dylib 2>/dev/null | head -n 1)
        if [ -f "$plugin_file" ]; then
            cp "$plugin_file" "$GST_PLUGIN_DST/"
            chmod 644 "$GST_PLUGIN_DST/$(basename "$plugin_file")"
            GST_COUNT=$((GST_COUNT + 1))
        else
            echo "       WARNING: GStreamer plugin not found: ${plugin_name}.dylib"
        fi
    done

    # Recursively walk ALL transitive deps of source plugins and copy missing ones to Frameworks.
    # Reuses collect_homebrew_deps (recursive otool walker defined earlier in this script).
    # Must use SOURCE plugins (not copied ones) because paths haven't been rewritten yet.
    VISITED_DEPS=$(mktemp /tmp/gst_visited.XXXXXX)
    COLLECTED_DEPS=$(mktemp /tmp/gst_collected.XXXXXX)
    for plugin_name in "${GST_REQUIRED_PLUGINS[@]}"; do
        src_plugin="$GST_PLUGIN_SRC/${plugin_name}.dylib"
        [ -f "$src_plugin" ] || continue
        collect_homebrew_deps "$src_plugin"
    done

    GST_DEP_COUNT=0
    while IFS=$'\t' read -r _orig resolved; do
        fname=$(basename "$resolved")
        if [ ! -f "$FRAMEWORKS_DIR/$fname" ]; then
            cp "$resolved" "$FRAMEWORKS_DIR/$fname"
            chmod 644 "$FRAMEWORKS_DIR/$fname"
            install_name_tool -id "@loader_path/$fname" "$FRAMEWORKS_DIR/$fname" 2>/dev/null || true
            rewrite_homebrew_refs "$FRAMEWORKS_DIR/$fname" "@loader_path"
            GST_DEP_COUNT=$((GST_DEP_COUNT + 1))
            echo "       + $fname (GStreamer plugin dependency)"
        fi
    done < <(sort -t$'\t' -k2 -u "$COLLECTED_DEPS")
    rm -f "$VISITED_DEPS" "$COLLECTED_DEPS"
    echo "       Copied $GST_DEP_COUNT additional dylibs for GStreamer plugins"

    # Now rewrite Homebrew paths in each copied plugin
    # Plugins are in Resources/gstreamer-1.0/, shared libs are in Frameworks/
    # Path from plugin to Frameworks: ../../Frameworks (up from gstreamer-1.0/ to Resources/ to Contents/ then into Frameworks/)
    GST_RPATH="@loader_path/../../Frameworks"
    for plugin in "$GST_PLUGIN_DST"/*.dylib; do
        [ -f "$plugin" ] || continue
        fname=$(basename "$plugin")
        install_name_tool -id "@loader_path/$fname" "$plugin" 2>/dev/null || true
        rewrite_homebrew_refs "$plugin" "$GST_RPATH"
    done

    echo "       Bundled $GST_COUNT GStreamer plugins"
else
    echo "       WARNING: GStreamer plugins not found at $GST_PLUGIN_SRC"
    echo "       Install with: brew install gstreamer"
fi

# Step 5: Sign, notarize, and create DMG
echo "[5/5] Signing and packaging..."

IDENTITY="$APPLE_SIGNING_IDENTITY"
ENTITLEMENTS="$SCRIPT_DIR/entitlements.plist"

# Clear extended attributes
xattr -cr "$APP_BUNDLE"

if [ "$DEV_MODE" = true ]; then
    # Ad-hoc signing for development
    echo "       Signing with ad-hoc signature (dev mode)..."

    # Sign all dylibs (Frameworks + GStreamer plugins in Resources)
    find "$FRAMEWORKS_DIR" "$RESOURCES_DIR/gstreamer-1.0" -name "*.dylib" -type f 2>/dev/null | while read -r fw; do
        codesign --force --sign - --entitlements "$ENTITLEMENTS" "$fw"
    done

    # Sign app bundle
    codesign --force --deep --sign - --entitlements "$ENTITLEMENTS" "$APP_BUNDLE"

    echo ""
    echo "=== Build Complete (Dev Mode) ==="
    echo "App bundle: $APP_BUNDLE"
    echo ""
    echo "To run:"
    echo "  open \"$APP_BUNDLE\""
    exit 0
fi

# Production signing with Developer ID
echo "       Signing all Mach-O binaries with Developer ID..."

# Sign any Mach-O binaries inside the app bundle
while IFS= read -r -d '' f; do
    if file "$f" | grep -q "Mach-O"; then
        echo "         codesign: $(basename "$f")"
        codesign --force --sign "$IDENTITY" --options runtime --timestamp "$f"
    fi
done < <(find "$APP_BUNDLE/Contents" -type f -print0)

# Sign the entire app bundle with hardened runtime + entitlements
echo "       Signing app bundle with hardened runtime..."
codesign --force --sign "$IDENTITY" \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    --timestamp \
    "$APP_BUNDLE"

echo "       Verifying codesign..."
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

# Notarize the app
ZIP_PATH="$SCRIPT_DIR/target/release/bundle/REACT-by-Compeller-ai.zip"
echo "       Creating notarization zip..."
rm -f "$ZIP_PATH" 2>/dev/null || true
ditto -c -k --keepParent "$APP_BUNDLE" "$ZIP_PATH"

echo "       Submitting to Apple notarization (this can take a while)..."
xcrun notarytool submit "$ZIP_PATH" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_PASSWORD" \
    --wait

echo "       Stapling notarization ticket..."
xcrun stapler staple "$APP_BUNDLE"

# Clean up zip
rm -f "$ZIP_PATH"

# Create DMG
DMG_DIR="$SCRIPT_DIR/target/release/bundle/dmg"
mkdir -p "$DMG_DIR"
DMG_PATH="$DMG_DIR/REACT-by-Compeller-ai.dmg"

VOLBASE="REACT Installer"
STAMP=$(date +%Y%m%d-%H%M%S)

cleanup_mounts() {
    for v in "/Volumes/REACT" "/Volumes/REACT Installer"; do
        if [ -d "$v" ]; then
            echo "       Detaching stale mount: $v"
            hdiutil detach "$v" >/dev/null 2>&1 || hdiutil detach -force "$v" >/dev/null 2>&1 || true
        fi
    done
    ls -1d /Volumes/"$VOLBASE"* 2>/dev/null | while IFS= read -r v; do
        [ -d "$v" ] || continue
        echo "       Detaching stale mount: $v"
        hdiutil detach "$v" >/dev/null 2>&1 || hdiutil detach -force "$v" >/dev/null 2>&1 || true
    done
}

# Stage the app into /tmp
STAGE_DIR=$(mktemp -d /tmp/react_dmg_stage.XXXXXX)
trap 'rm -rf "$STAGE_DIR"' EXIT

echo "       Staging app for DMG..."
ditto "$APP_BUNDLE" "$STAGE_DIR/$(basename "$APP_BUNDLE")"

cleanup_mounts

echo "       Creating DMG: $DMG_PATH"
rm -f "$DMG_PATH" 2>/dev/null || true

CREATED=0
for attempt in 1 2 3; do
    VOLNAME="$VOLBASE $STAMP-$attempt"
    echo "       DMG attempt $attempt (volname: $VOLNAME)"
    if hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE_DIR" -ov -format UDZO "$DMG_PATH"; then
        CREATED=1
        break
    fi
    echo "       hdiutil failed; cleaning mounts and retrying..."
    cleanup_mounts
    rm -f "$DMG_PATH" 2>/dev/null || true
    sleep 1
    STAMP=$(date +%Y%m%d-%H%M%S)
done

if [ "$CREATED" -ne 1 ]; then
    echo "ERROR: DMG creation failed."
    echo "If you saw 'Operation not permitted':"
    echo "  1) Close any Finder windows showing /Volumes/REACT*"
    echo "  2) System Settings > Privacy & Security > Full Disk Access > enable Terminal"
    echo "  3) Re-run the script"
    exit 1
fi

echo "       Verifying Gatekeeper acceptance..."
spctl -a -vv "$APP_BUNDLE" || true

echo ""
echo "=== Build Complete ==="
echo "App bundle: $APP_BUNDLE"
echo "DMG: $DMG_PATH"
echo ""
echo "To test install:"
echo "  open \"$DMG_PATH\""
