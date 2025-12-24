#!/bin/bash
# Run fracturize, wait for render, take screenshot

cargo build --release 2>&1

./target/release/fracturize &
APP_PID=$!

sleep 2

# Get window ID by name and capture it
WIN_ID=$(xdotool search --name "Fracturize" | head -1)
if [ -n "$WIN_ID" ]; then
    import -window "$WIN_ID" /tmp/fracturize_screenshot.png
else
    echo "Could not find Fracturize window, capturing full screen"
    import -window root /tmp/fracturize_screenshot.png
fi

kill $APP_PID 2>/dev/null || true

echo "Screenshot saved to /tmp/fracturize_screenshot.png"
