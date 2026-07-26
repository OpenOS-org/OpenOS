#!/bin/bash
# tests/integration.sh — QEMU-based integration tests for OpenOS
#
# Boots the kernel in QEMU, captures serial output, and verifies
# expected strings appear. Returns 0 on success, 1 on failure.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
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

log() { echo -e "${GREEN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; FAILURES=$((FAILURES + 1)); }

FAILURES=0

echo "========================================="
echo "  OpenOS Integration Tests"
echo "========================================="
echo ""

# Build
echo "--- Building kernel and disk image ---"
make build > /dev/null 2>&1
if [ $? -ne 0 ]; then
    fail "Build failed"
    exit 1
fi
log "Build successful"
echo ""

# Run QEMU
echo "--- Running QEMU (timeout: ${TIMEOUT}s) ---"
timeout "$TIMEOUT" qemu-system-x86_64 \
    -drive format=raw,file="$BIOS_IMG" \
    -serial file:"$SERIAL_LOG" \
    -display none \
    -no-reboot 2>/dev/null || true

OUTPUT=$(cat "$SERIAL_LOG" 2>/dev/null || echo "")

if [ -z "$OUTPUT" ]; then
    fail "No serial output captured"
    exit 1
fi

echo ""
echo "--- Verifying boot sequence ---"

# Test 1: Kernel banner
if echo "$OUTPUT" | grep -q "OpenOS Microkernel"; then
    log "Kernel banner present"
else
    fail "Kernel banner missing"
fi

# Test 2: IPC initialization
if echo "$OUTPUT" | grep -q "IPC subsystem initialized"; then
    log "IPC subsystem initialized"
else
    fail "IPC initialization failed"
fi

# Test 3: Kernel initialization complete
if echo "$OUTPUT" | grep -q "Kernel initialization complete"; then
    log "Kernel initialization complete"
else
    fail "Kernel initialization incomplete"
fi

# Test 4: Ramdisk loaded
if echo "$OUTPUT" | grep -q "Ramdisk loaded"; then
    log "Ramdisk loaded"
else
    fail "Ramdisk not loaded"
fi

# Test 5: Channel created
if echo "$OUTPUT" | grep -q "Channel:"; then
    log "Channel created"
else
    fail "Channel creation failed"
fi

# Test 6: Console service launched
if echo "$OUTPUT" | grep -q "Launching console service"; then
    log "Console service launched"
else
    fail "Console service not launched"
fi

# Test 7: ELF loading
if echo "$OUTPUT" | grep -q "ELF loaded"; then
    log "ELF binary loaded"
else
    fail "ELF loading failed"
fi

# Test 8: User-space message (the key test!)
if echo "$OUTPUT" | grep -q "Hello from user-space"; then
    log "User-space message received via Channel IPC"
else
    fail "User-space message NOT received"
fi

# Test 9: Clean exit
if echo "$OUTPUT" | grep -q "SYS_EXIT.*status=0"; then
    log "User process exited cleanly"
else
    fail "User process did not exit cleanly"
fi

# Test 10: No panics
echo ""
echo "--- Verifying no panics ---"
if echo "$OUTPUT" | grep -q "PANIC"; then
    fail "Kernel panic detected:"
    echo "$OUTPUT" | grep "PANIC"
else
    log "No kernel panics"
fi

# Test 11: No double faults
if echo "$OUTPUT" | grep -q "DOUBLE FAULT"; then
    fail "Double fault detected"
else
    log "No double faults"
fi

# Summary
echo ""
echo "========================================="
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}All integration tests passed!${NC}"
    echo ""
    echo "Serial output:"
    echo "$OUTPUT" | grep -v "^INFO" | grep -v "^$" | head -30
else
    echo -e "${RED}$FAILURES test(s) failed${NC}"
fi
echo "========================================="

exit $FAILURES
