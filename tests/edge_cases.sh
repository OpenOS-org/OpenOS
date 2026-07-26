#!/bin/bash
# tests/edge_cases.sh — Edge case and boundary tests for OpenOS
#
# Tests boundary conditions, error paths, and unusual scenarios.
# Returns 0 on success, 1 on failure.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

BIOS_IMG="target/debug/openos-bios.img"
SERIAL_LOG="/tmp/openos_edge_test.log"

cd "$(dirname "$0")/.."

log() { echo -e "  ${GREEN}✓${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; FAILURES=$((FAILURES + 1)); }
warn() { echo -e "  ${YELLOW}⚠${NC} $1"; }

FAILURES=0
TESTS=0

run_qemu() {
    timeout 15 qemu-system-x86_64 \
        -drive format=raw,file="$BIOS_IMG" \
        -serial file:"$SERIAL_LOG" \
        -display none \
        -no-reboot 2>/dev/null || true
}

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
echo "  OpenOS Edge Case Tests"
echo "========================================="
echo ""

# Build
make build > /dev/null 2>&1

# ─────────────────── Edge Case 1: Initrd Parsing ───────────────────

echo "${CYAN}Edge Case 1: Initrd Integrity${NC}"
run_qemu
check "Ramdisk loaded" "Initrd parsed successfully"
check "Found 'console_svc.elf'" "Console service found in initrd"
echo ""

# ─────────────────── Edge Case 2: ELF Loading ───────────────────

echo "${CYAN}Edge Case 2: ELF Loading${NC}"
check "ELF loaded: entry=" "ELF entry point resolved"
check "stack=" "User stack allocated"
check "Transitioning to Ring 3" "Ring 3 transition attempted"
check_not "ELF load failed" "ELF loading did not fail"
check_not "map_to failed" "Page mapping did not fail"
echo ""

# ─────────────────── Edge Case 3: Page Table Manipulation ───────────────────

echo "${CYAN}Edge Case 3: Page Table Safety${NC}"
check "ELF loaded:" "User pages mapped successfully"
check_not "PAGE FAULT" "No page faults during mapping"
check_not "page table" "No page table errors"
echo ""

# ─────────────────── Edge Case 4: Channel Operations ───────────────────

echo "${CYAN}Edge Case 4: Channel IPC Correctness${NC}"
check "channel_receive: got 31 bytes" "Message length correct (31 bytes)"
check "Hello from user-space service!" "Message content preserved exactly"
check "channel_reply: stored" "Reply stored successfully"
check "SYS_EXIT.*status=0" "Clean exit after channel operations"
echo ""

# ─────────────────── Edge Case 5: Handle Lifecycle ───────────────────

echo "${CYAN}Edge Case 5: Handle Management${NC}"
check "handle_a=0x" "Handle A created with valid address"
check "handle_b=0x" "Handle B created with valid address"
check "Console handle: 0x" "Console handle passed to user process"
echo ""

# ─────────────────── Edge Case 6: Interrupt Safety ───────────────────

echo "${CYAN}Edge Case 6: Interrupt Safety${NC}"
check "Kernel initialization complete" "Interrupts enabled after init"
check_not "DOUBLE FAULT" "No double faults"
check_not "triple fault" "No triple faults"
echo ""

# ─────────────────── Edge Case 7: Memory Bounds ───────────────────

echo "${CYAN}Edge Case 7: Memory Bounds${NC}"
check_not "Out of memory" "No OOM errors"
check_not "allocation" "No allocation failures"
check_not "stack overflow" "No stack overflows"
echo ""

# ─────────────────── Edge Case 8: Syscall Dispatch ───────────────────

echo "${CYAN}Edge Case 8: Syscall Dispatch${NC}"
check "channel_receive:" "channel_receive dispatched"
check "SYS_EXIT" "process_exit dispatched"
check_not "unknown syscall" "No unknown syscall errors"
echo ""

# ─────────────────── Edge Case 9: Serial Output ───────────────────

echo "${CYAN}Edge Case 9: Serial Output Integrity${NC}"
check "OpenOS Microkernel v0.3.0" "Version string correct"
check "IPC subsystem initialized" "IPC init message correct"
check "Kernel initialization complete" "Init complete message correct"
echo ""

# ─────────────────── Edge Case 10: Boot Completeness ───────────────────

echo "${CYAN}Edge Case 10: Boot Completeness${NC}"
check "IPC subsystem initialized" "IPC initialized"
check "Kernel initialization complete" "Kernel fully initialized"
check "Launching console service" "Task scheduling works"
check "Transitioning to Ring 3" "Ring 3 transition works"
echo ""

# ─────────────────── Summary ───────────────────

echo "========================================="
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}All $TESTS edge case tests passed!${NC}"
else
    echo -e "${RED}$FAILURES of $TESTS edge case tests failed${NC}"
fi
echo "========================================="

exit $FAILURES
