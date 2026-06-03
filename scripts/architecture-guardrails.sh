#!/bin/bash
# architecture-guardrails.sh - Verify architecture boundaries
# Idempotent and safe to run multiple times.
set -e

echo "=== Architecture Guardrails ==="
FAILURES=0

# Resolve repo root (parent of scripts/)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# 1. Helper V2 must not contain password/cookie/token
echo -n "Checking helper for forbidden fields... "
if grep -ri "password\|cookie\|webvpn_session\|auth_token" src/helper_common/ src/helper_runtime/ 2>/dev/null | grep -v "// " | grep -v "test" | grep -v "\.md"; then
    echo "FAIL: Found forbidden fields in helper code"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# 2. Helper V2 must not include vpn_engine/protocol
echo -n "Checking helper for protocol includes... "
if grep -r "vpn_engine/protocol" src/helper_common/ src/helper_runtime/ 2>/dev/null; then
    echo "FAIL: Found protocol includes in helper code"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# 3. Core must not have platform #ifdef
echo -n "Checking core for platform ifdef... "
if grep -r "#ifdef _WIN32\|#ifdef __APPLE__\|#if defined(_WIN32)\|#if defined(__APPLE__)" src/core/ 2>/dev/null; then
    echo "FAIL: Found platform ifdef in core"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# 4. New UI path must not have retry_limit
echo -n "Checking new UI path for retry_limit... "
if grep -r "retry_limit" src/core_api/ 2>/dev/null; then
    echo "FAIL: Found retry_limit in new core_api"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# 5. No secrets in test fixtures
echo -n "Checking test fixtures for secrets... "
if grep -ri "password.*=.*['\"]" tests/ 2>/dev/null | grep -v "no_secret\|forbidden\|should_not\|must_not\|verify_no\|// " | head -5; then
    echo "WARNING: Possible secrets in test fixtures"
else
    echo "PASS"
fi

# 6. Helper must not include core/ headers
echo -n "Checking helper for core includes... "
if grep -r '#include.*"core/' src/helper_common/ src/helper_runtime/ 2>/dev/null; then
    echo "FAIL: Found core includes in helper code"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# 7. Platform ops must not include protocol/auth headers
echo -n "Checking platform ops for protocol includes... "
if grep -r "vpn_engine/protocol\|native_auth" src/platform/ 2>/dev/null; then
    echo "FAIL: Found protocol includes in platform code"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

echo ""
if [ $FAILURES -gt 0 ]; then
    echo "=== $FAILURES guardrail violations found ==="
    exit 1
else
    echo "=== All guardrails passed ==="
    exit 0
fi
