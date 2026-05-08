#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# ECNU-VPN Linux Install Script
#
# Installs the exv binary to /usr/local/bin/exv with setuid root permissions,
# and ensures openconnect is available as a system dependency.
# Safe to run multiple times (idempotent).
# ---------------------------------------------------------------------------

readonly BUILD_DIR="$(cd "$(dirname "$0")/../build" && pwd)"
readonly INSTALL_PATH="/usr/local/bin/exv"
readonly SCRIPT_NAME="$(basename "$0")"

# Colors for status messages
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
DIM='\033[2m'
RESET='\033[0m'

print_status()  { printf "  ${BLUE}→${RESET} %s\n" "$1"; }
print_success() { printf "  ${GREEN}✓${RESET} %s\n" "$1"; }
print_warning() { printf "  ${YELLOW}⚠${RESET}  %s\n" "$1"; }
print_error()   { printf "  ${RED}✗${RESET} %s\n" "$1" >&2; }
print_header()  { printf "\n${BLUE}═══ %s ═══${RESET}\n\n" "$1"; }

# ---------------------------------------------------------------------------
# Detect the system package manager
# ---------------------------------------------------------------------------
detect_pkg_manager() {
    if command -v apt-get >/dev/null 2>&1; then
        echo "apt-get"
    elif command -v dnf >/dev/null 2>&1; then
        echo "dnf"
    elif command -v pacman >/dev/null 2>&1; then
        echo "pacman"
    else
        echo ""
    fi
}

# ---------------------------------------------------------------------------
# Ensure openconnect is installed
# ---------------------------------------------------------------------------
install_openconnect() {
    print_status "Checking for openconnect..."

    if command -v openconnect >/dev/null 2>&1; then
        print_success "openconnect is already installed ($(command -v openconnect))."
        return 0
    fi

    local pkg_manager
    pkg_manager="$(detect_pkg_manager)"

    if [ -z "$pkg_manager" ]; then
        print_error "No supported package manager found (apt-get, dnf, pacman)."
        print_error "Please install openconnect manually, then re-run this script."
        return 1
    fi

    print_status "openconnect not found. Installing via ${pkg_manager}..."

    case "$pkg_manager" in
        apt-get)
            apt-get update -qq
            apt-get install -y -qq openconnect
            ;;
        dnf)
            dnf install -y -q openconnect
            ;;
        pacman)
            pacman -S --noconfirm --needed openconnect
            ;;
    esac

    if command -v openconnect >/dev/null 2>&1; then
        print_success "openconnect installed successfully."
    else
        print_error "Failed to install openconnect."
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Install the exv binary
# ---------------------------------------------------------------------------
install_exv_binary() {
    local source_binary="${BUILD_DIR}/exv"

    # Check that the build artifact exists
    if [ ! -f "$source_binary" ]; then
        print_error "Build artifact not found: ${source_binary}"
        print_error "Run 'cmake --build build' first, then re-run this script."
        return 1
    fi

    print_status "Installing exv binary to ${INSTALL_PATH}..."

    # Idempotency: check if already installed with the same content
    if [ -f "$INSTALL_PATH" ] && cmp -s "$source_binary" "$INSTALL_PATH"; then
        print_success "exv binary is already up to date at ${INSTALL_PATH}."
    else
        if [ -f "$INSTALL_PATH" ]; then
            print_warning "Existing exv binary found. Replacing..."
        fi
        cp "$source_binary" "$INSTALL_PATH"
        print_success "exv binary copied to ${INSTALL_PATH}."
    fi

    # Set ownership and permissions
    chown root:root "$INSTALL_PATH" 2>/dev/null || print_warning "Could not set root:root ownership (already root?)."
    chmod 4755 "$INSTALL_PATH"
    print_success "Setuid root permission set on ${INSTALL_PATH}."

    # Verify the installed binary works
    if [ -x "$INSTALL_PATH" ]; then
        local installed_version
        installed_version="$("$INSTALL_PATH" version 2>/dev/null || echo "unknown")"
        print_success "Installed binary version: ${installed_version}"
    else
        print_warning "Installed binary is not executable."
    fi
}

# ---------------------------------------------------------------------------
# Verify permissions include setuid bit
# ---------------------------------------------------------------------------
verify_setuid() {
    local perms
    perms="$(stat -c '%a' "$INSTALL_PATH" 2>/dev/null || echo "")"
    if [ "${perms:0:1}" = "4" ]; then
        print_success "Setuid bit confirmed (mode: ${perms})."
    else
        print_warning "Setuid bit may not be set (mode: ${perms})."
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    print_header "ECNU-VPN Linux Installer"

    # Require root for installation
    if [ "$(id -u)" -ne 0 ]; then
        print_error "This script must be run as root (sudo)."
        exit 1
    fi

    # Step 1: Install openconnect dependency
    install_openconnect || exit 1

    # Step 2: Install exv binary
    install_exv_binary || exit 1

    # Step 3: Verify setuid
    verify_setuid

    echo
    print_success "ECNU-VPN installation complete."
    print_status "Next step: run 'sudo exv service install' to set up the systemd helper daemon."
    print_status "After that, you can use 'exv' and 'exv stop' without sudo."
    echo

    return 0
}

main "$@"
