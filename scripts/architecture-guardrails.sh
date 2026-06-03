#!/bin/bash
# architecture-guardrails.sh - Verify architecture boundaries
# Idempotent and safe to run multiple times.
set -e

echo "=== Architecture Guardrails ==="
FAILURES=0
WARNINGS=0

# Resolve repo root (parent of scripts/)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Allowlist loading
# ---------------------------------------------------------------------------
ALLOWLIST_FILE="docs/architecture/guardrail_allowlist.yml"
declare -A ALLOWLIST

load_allowlist() {
    if [ ! -f "$ALLOWLIST_FILE" ]; then
        echo "WARNING: Allowlist file not found at $ALLOWLIST_FILE"
        return
    fi

    local current_file=""
    local current_rule=""
    while IFS= read -r line; do
        if echo "$line" | grep -qE '^\s+- file:'; then
            current_file=$(echo "$line" | sed 's/.*file:\s*//' | xargs)
        elif echo "$line" | grep -qE '^\s+rule:'; then
            current_rule=$(echo "$line" | sed 's/.*rule:\s*//' | xargs)
            if [ -n "$current_file" ] && [ -n "$current_rule" ]; then
                ALLOWLIST["${current_file}|${current_rule}"]=1
            fi
        fi
    done < "$ALLOWLIST_FILE"
}

load_allowlist
echo "Loaded ${#ALLOWLIST[@]} allowlist entries from guardrail_allowlist.yml"

# Helper: check if a file+rule pair is allowlisted
is_allowlisted() {
    local file_path="$1"
    local rule="$2"
    local relative="${file_path#$REPO_ROOT/}"
    relative="${relative#\\}"
    local key="${relative}|${rule}"
    [ "${ALLOWLIST[$key]+_}" ] && return 0 || return 1
}

# ---------------------------------------------------------------------------
# Rule 1: Helper V2 must not contain password/cookie/token
# ---------------------------------------------------------------------------
echo -n "Checking helper for forbidden fields... "
found=0
while IFS= read -r match; do
    line_content=$(echo "$match" | cut -d: -f2-)
    skip=0
    echo "$line_content" | grep -qE '^\s*//' && skip=1
    echo "$line_content" | grep -qi 'test' && skip=1
    echo "$line_content" | grep -q '\.md' && skip=1
    if [ $skip -eq 0 ]; then
        echo ""
        echo "  $match"
        found=1
    fi
done < <(grep -rin "password\|cookie\|webvpn_session\|auth_token" src/helper_common/ src/helper_runtime/ 2>/dev/null || true)
if [ $found -eq 1 ]; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 2: Helper V2 must not include vpn_engine/protocol
# ---------------------------------------------------------------------------
echo -n "Checking helper for protocol includes... "
if grep -r "vpn_engine/protocol" src/helper_common/ src/helper_runtime/ 2>/dev/null; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 3: Core must not have platform #ifdef
# ---------------------------------------------------------------------------
echo -n "Checking core for platform ifdef... "
if grep -r "#ifdef _WIN32\|#ifdef __APPLE__\|#if defined(_WIN32)\|#if defined(__APPLE__)" src/core/ 2>/dev/null; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 4: New UI path must not have retry_limit
# ---------------------------------------------------------------------------
echo -n "Checking new UI path for retry_limit... "
if grep -r "retry_limit" src/core_api/ 2>/dev/null; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 5: No secrets in test fixtures (FAIL, not WARNING)
#
# Allowed tokens: MOCK_PASSWORD, test-mock, placeholder, example.invalid
# ---------------------------------------------------------------------------
echo -n "Checking test fixtures for secrets... "
secret_found=0
while IFS= read -r match; do
    [ -z "$match" ] && continue
    line_content=$(echo "$match" | cut -d: -f2-)
    skip=0
    echo "$line_content" | grep -q "no_secret" && skip=1
    echo "$line_content" | grep -q "forbidden" && skip=1
    echo "$line_content" | grep -q "should_not" && skip=1
    echo "$line_content" | grep -q "must_not" && skip=1
    echo "$line_content" | grep -q "verify_no" && skip=1
    echo "$line_content" | grep -q "// " && skip=1
    echo "$line_content" | grep -q "MOCK_PASSWORD" && skip=1
    echo "$line_content" | grep -q "test-mock" && skip=1
    echo "$line_content" | grep -q "placeholder" && skip=1
    echo "$line_content" | grep -q "example\.invalid" && skip=1
    echo "$line_content" | grep -q '\.find(' && skip=1
    echo "$line_content" | grep -q "contains_secret" && skip=1
    echo "$line_content" | grep -q "EXPECT FAILED" && skip=1
    if [ $skip -eq 0 ]; then
        echo ""
        echo "  $match"
        secret_found=1
    fi
done < <(grep -rinP "password\s*=\s*['\"]" tests/ 2>/dev/null || true)
if [ $secret_found -eq 1 ]; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 6: Helper must not include core/ headers
# ---------------------------------------------------------------------------
echo -n "Checking helper for core includes... "
if grep -r '#include.*"core/' src/helper_common/ src/helper_runtime/ 2>/dev/null; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 7: Platform ops must not include protocol/auth headers
#           Allowlisted entries are documented in guardrail_allowlist.yml
# ---------------------------------------------------------------------------
echo -n "Checking platform ops for protocol includes... "
platform_found=0
platform_allowlisted=0
while IFS= read -r match; do
    [ -z "$match" ] && continue
    file_path=$(echo "$match" | cut -d: -f1)
    if is_allowlisted "$file_path" "protocol_include_in_platform"; then
        platform_allowlisted=$((platform_allowlisted + 1))
    else
        echo ""
        echo "  $match"
        platform_found=1
    fi
done < <(grep -rn "vpn_engine/protocol\|native_auth" src/platform/ 2>/dev/null || true)
if [ $platform_found -eq 1 ]; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
elif [ $platform_allowlisted -gt 0 ]; then
    echo "PASS (allowlisted: $platform_allowlisted hits)"
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Rule 8: Non-protocol app_api files must not include protocol headers
#           Allowlisted entries are documented in guardrail_allowlist.yml
# ---------------------------------------------------------------------------
echo -n "Checking app_api for protocol includes... "
appapi_found=0
appapi_allowlisted=0
if [ -f "src/app_api_native_orchestration.hpp" ]; then
    while IFS= read -r match; do
        [ -z "$match" ] && continue
        if is_allowlisted "src/app_api_native_orchestration.hpp" "protocol_include_in_non_protocol"; then
            appapi_allowlisted=$((appapi_allowlisted + 1))
        else
            echo ""
            echo "  $match"
            appapi_found=1
        fi
    done < <(grep -n "vpn_engine/protocol" src/app_api_native_orchestration.hpp 2>/dev/null || true)
fi
if [ $appapi_found -eq 1 ]; then
    echo "FAIL"
    FAILURES=$((FAILURES + 1))
elif [ $appapi_allowlisted -gt 0 ]; then
    echo "PASS (allowlisted: $appapi_allowlisted hits)"
else
    echo "PASS"
fi

# ---------------------------------------------------------------------------
# Allowlist summary
# ---------------------------------------------------------------------------
echo ""
echo "=== Allowlist Summary ==="
if [ ${#ALLOWLIST[@]} -eq 0 ]; then
    echo "  (no allowlist entries loaded)"
else
    for key in "${!ALLOWLIST[@]}"; do
        file="${key%%|*}"
        rule="${key#*|}"
        if [ -f "$file" ]; then
            status="active"
        else
            status="STALE (file missing)"
        fi
        echo "  [$status] $file ($rule)"
    done
fi

# ---------------------------------------------------------------------------
# Final summary
# ---------------------------------------------------------------------------
echo ""
if [ $FAILURES -gt 0 ]; then
    echo "=== $FAILURES guardrail violation(s) found, $WARNINGS warning(s) ==="
    exit 1
elif [ $WARNINGS -gt 0 ]; then
    echo "=== All guardrails passed ($WARNINGS warning(s)) ==="
    exit 0
else
    echo "=== All guardrails passed ==="
    exit 0
fi
