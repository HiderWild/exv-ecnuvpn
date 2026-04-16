#!/bin/zsh

# 🔌 Stop VPN - Terminate openconnect VPN connection

echo "🔍 Searching for openconnect processes..."

# Find openconnect process
VPN_PID=$(pgrep -x openconnect)

if [ -z "$VPN_PID" ]; then
    echo "❌ No openconnect process found. VPN is not running."
    exit 1
fi

echo "✅ Found openconnect process: PID $VPN_PID"
echo "🛑 Stopping VPN connection..."

# Kill the process
sudo kill $VPN_PID

# Wait a moment and verify
sleep 1

if pgrep -x openconnect > /dev/null; then
    echo "⚠️  Process still running, force killing..."
    sudo kill -9 $VPN_PID
    sleep 1
fi

# Final check
if pgrep -x openconnect > /dev/null; then
    echo "❌ Failed to stop VPN connection"
    exit 1
else
    echo "✅ VPN connection stopped successfully! 🎉"
    exit 0
fi
