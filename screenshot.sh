#!/bin/bash
# Run fracturize with built-in screenshot capture

cargo build --release 2>&1

./target/release/fracturize --screenshot --delay 500

echo "Screenshot saved to screenshots/capture.png"
