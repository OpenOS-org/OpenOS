#!/bin/bash
# tests/integration.sh — QEMU-based integration tests for OpenOS
#
# Boots the kernel in QEMU, captures serial output, and verifies
# expected strings appear. Returns 0 on success, 1 on failure.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

TIMEOUT=15
BIOS_IMG="target/debug/openos-bios.img"
SERIAL_LOG="/tmp/openos_integration_test.log"

cd "$(dirname "$0")/.."

for arg in "$@"; do
    case $arg in
        --timeout=*) TIMEOUT="${arg#*=}" ;;
    esac
done

log() { echo -e "  ${GREEN}✓${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; FAILURES=$((FAILURES + 1)); }

FAILURES=0
TESTS=0

check() {
    local pattern="$1"
    local desc="$2"
    TESTS=$((TESTS + 1))
    if grep -q "$pattern" "$SERIAL_LOG" 2>/dev/null; then
        log "$desc"
    else
        fail "$desc"
    fi
}

check_not() {
    local pattern="$1"
    local desc="$2"
    TESTS=$((TESTS + 1))
    if grep -q "$pattern" "$SERIAL_LOG" 2>/dev/null; then
        fail "$desc (found: $pattern)"
    else
        log "$desc"
    fi
}

echo "========================================="
echo "  OpenOS Integration Tests"
echo "========================================="
echo ""

# Build (skip if disk image already exists, e.g. CI downloaded artifact)
echo "--- Building ---"
if [ -f "$BIOS_IMG" ]; then
    log "Using existing disk image: $BIOS_IMG"
else
    make build > /dev/null 2>&1
    if [ $? -ne 0 ]; then
        fail "Build failed"
        exit 1
    fi
    log "Build successful"
fi
echo ""

# Run QEMU
echo "--- Running QEMU (timeout: ${TIMEOUT}s) ---"
timeout "$TIMEOUT" qemu-system-x86_64 \
    -drive format=raw,file="$BIOS_IMG" \
    -serial file:"$SERIAL_LOG" \
    -display none \
    -no-reboot 2>/dev/null || true

if [ ! -s "$SERIAL_LOG" ]; then
    fail "No serial output captured"
    exit 1
fi

echo ""
echo "--- Boot sequence ---"
check "OpenOS Microkernel" "Kernel banner"
check "IPC subsystem initialized" "IPC initialized"
check "Kernel initialization complete" "Kernel fully initialized"
check "Ramdisk loaded" "Ramdisk loaded"
check "Channel:" "Channel created"
check "Launching console service" "Console service launched"
check "ELF loaded" "ELF binary loaded"

echo ""
echo "--- IPC ---"
check "channel_receive" "channel_receive executed"
check "channel_receive: got" "Message received"
check "Hello from user-space" "Message content correct"
check "channel_reply" "Reply sent"
check "SYS_EXIT.*status=0" "Clean exit"

echo ""
echo "--- Safety ---"
check_not "PANIC" "No kernel panics"
check_not "DOUBLE FAULT" "No double faults"
check_not "PAGE FAULT" "No page faults"

echo ""
echo "========================================="
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}All $TESTS integration tests passed!${NC}"
else
    echo -e "${RED}$FAILURES of $TESTS tests failed${NC}"
fi
echo "========================================="

exit $FAILURES
