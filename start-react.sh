#!/bin/bash
# Build and run REACT from source.
# Usage: ./start-react.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# PATH, pkg-config, NDI_SDK_DIR, .env.local, path-remapping rustflags
source "$SCRIPT_DIR/scripts/env.sh"

echo "=== REACT by Compeller.ai ==="

# Runtime library paths for ffmpeg and NDI
export DYLD_LIBRARY_PATH="/opt/homebrew/opt/ffmpeg/lib:$NDI_SDK_DIR/lib:$DYLD_LIBRARY_PATH"

# Check for custom ffmpeg with NDI support
FFMPEG_DIR="$SCRIPT_DIR/ffmpeg"
EMBEDDED_FFMPEG="$FFMPEG_DIR/ffmpeg"

if [ -f "$EMBEDDED_FFMPEG" ]; then
    echo "Using embedded ffmpeg with NDI support"
    export PATH="$FFMPEG_DIR:$PATH"
    # Add ffmpeg libs to library path
    if [ -d "$FFMPEG_DIR/lib" ]; then
        export DYLD_LIBRARY_PATH="$FFMPEG_DIR/lib:$DYLD_LIBRARY_PATH"
    fi
elif [ -f "/opt/homebrew/bin/ffmpeg" ]; then
    echo "Using system ffmpeg (no NDI support)"
else
    echo "WARNING: ffmpeg not found. Streaming/recording will not work."
    echo "         Install with: brew install ffmpeg"
    echo "         Or place custom NDI-enabled ffmpeg in: $FFMPEG_DIR/"
fi

if [ ! -f "$NDI_SDK_DIR/lib/libndi.dylib" ] && [ ! -f "$NDI_SDK_DIR/lib/macOS/libndi.dylib" ]; then
    echo "NOTE: NDI SDK not found at $NDI_SDK_DIR — NDI output will be disabled"
fi

# Check for the camera detection model (models/rfdetr-nano.onnx, or another
# rfdetr-*.onnx / dfine-*.onnx export)
MODELS_DIR="$SCRIPT_DIR/models"
VISION_MODEL="$MODELS_DIR/rfdetr-nano.onnx"
if [ ! -f "$VISION_MODEL" ]; then
    for candidate in "$MODELS_DIR"/rfdetr-*.onnx "$MODELS_DIR"/dfine-*.onnx; do
        if [ -f "$candidate" ]; then VISION_MODEL="$candidate"; break; fi
    done
fi

if [ -f "$VISION_MODEL" ]; then
    SIZE=$(du -h "$VISION_MODEL" | cut -f1)
    echo "Vision model ready: $(basename "$VISION_MODEL") ($SIZE)"
else
    echo "WARNING: Vision model not found in $MODELS_DIR"
    echo "         Camera tracking will be disabled"
    echo ""
    echo "To enable vision tracking, see: models/README.md"
fi

# Features:
#   camera - USB webcam tracking via nokhwa + ONNX
#   dev    - talk to the backend named by REACT_DEV_API_HOST (set in .env.local)
#            instead of https://compeller.ai
FEATURES="camera"
if [ -n "$REACT_DEV_API_HOST" ]; then
    FEATURES="camera,dev"
    echo "API host: $REACT_DEV_API_HOST (dev)"
else
    echo "API host: https://compeller.ai"
fi

# RUST_LOG: errors by default, plus the subsystems most often being diagnosed.
# Streaming diagnostics alternative:
#   RUST_LOG=error,compeller_react::output::ffmpeg_streamer=info,compeller_react::output::twitch_servers=info,compeller_react::app=info,compeller_react::compel_picker=info ./start-react.sh
export RUST_BACKTRACE=1
export RUST_LOG="${RUST_LOG:-error,compeller_react::set_sync=debug,compeller_react::auth=debug,compeller_react::track::abletonlink=info,compeller_react::dmx::show_brain=info}"

cargo run --release --features "$FEATURES" 2>&1
