#!/bin/bash
# Run fracturize with built-in screenshot capture
# Usage: ./screenshot.sh [scene_path]

SCENE="${1:-scenes/sierpinski.toml}"

cargo build --release 2>&1

echo "Capturing screenshot for scene: $SCENE"
./target/release/fracturize --scene "$SCENE" --screenshot --delay 500

echo "Screenshot saved to screenshots/capture.png"
