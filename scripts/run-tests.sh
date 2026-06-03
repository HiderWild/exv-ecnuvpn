#!/bin/bash
# run-tests.sh - Local test runner for ECNU-VPN
set -e

PRESET="${1:-linux-release}"
LABEL="${2:-}"

echo "=== ECNU-VPN Test Runner ==="
echo "Preset: $PRESET"

# Configure
echo -e "\n--- Configuring ---"
cmake --preset "$PRESET"

# Build
echo -e "\n--- Building ---"
cmake --build --preset "$PRESET"

# Test
echo -e "\n--- Running Tests ---"
CTEST_ARGS="--preset $PRESET --output-on-failure"
if [ -n "$LABEL" ]; then
    CTEST_ARGS="$CTEST_ARGS -L $LABEL"
fi
ctest $CTEST_ARGS || true

echo -e "\n=== Results ==="
echo "Check Testing/Temporary/LastTest.log for details"
