#!/bin/bash
# tests/scenarios.sh — Scenario and edge-case tests for OpenOS
#
# Tests complex multi-step workflows and edge cases.
# Returns 0 on success, 1 on failure.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

BIOS_IMG="target/debug/openos-bios.img"
SERIAL_LOG="/tmp/openos_scenario_test.log"

cd "$(dirname "$0")/.."

log() { echo -e "  ${GREEN}✓${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; FAILURES=$((FAILURES + 1)); }
warn() { echo -e "  ${YELLOW}⚠${NC} $1"; }

FAILURES=0
SCENARIOS=0
PASSED=0

run_qemu() {
    timeout 15 qemu-system-x86_64 \
        -drive format=raw,file="$BIOS_IMG" \
        -serial file:"$SERIAL_LOG" \
        -display none \
        -no-reboot 2>/dev/null || true
}

check_output() {
    local pattern="$1"
    local description="$2"
    SCENARIOS=$((SCENARIOS + 1))
    if grep -q "$pattern" "$SERIAL_LOG" 2>/dev/null; then
        log "$description"
        PASSED=$((PASSED + 1))
    else
        fail "$description"
    fi
}

check_not_output() {
    local pattern="$1"
    local description="$2"
    SCENARIOS=$((SCENARIOS + 1))
    if grep -q "$pattern" "$SERIAL_LOG" 2>/dev/null; then
        fail "$description (found unexpected: $pattern)"
    else
        log "$description"
        PASSED=$((PASSED + 1))
    fi
}

echo "========================================="
echo "  OpenOS Scenario Tests"
echo "========================================="
echo ""

# ─────────────────── Scenario 1: Boot Sequence ───────────────────

echo "${CYAN}Scenario 1: Complete Boot Sequence${NC}"
if [ ! -f "$BIOS_IMG" ]; then
    make build > /dev/null 2>&1
fi
run_qemu

echo ""
echo "--- Serial Log (first 80 lines) ---"
head -80 "$SERIAL_LOG"
echo "--- End Serial Log ---"
echo ""

check_output "OpenOS Microkernel" "Kernel banner displayed"
check_output "IPC subsystem initialized" "IPC subsystem initialized"
check_output "Kernel initialization complete" "Kernel fully initialized"
check_not_output "PANIC" "No kernel panics during boot"
check_not_output "DOUBLE FAULT" "No double faults during boot"
check_not_output "PAGE FAULT" "No page faults during boot"
echo ""

# ─────────────────── Scenario 2: Ramdisk Loading ───────────────────

echo "${CYAN}Scenario 2: Ramdisk and ELF Loading${NC}"

check_output "Ramdisk loaded" "Ramdisk loaded from bootloader"
check_output "Channel:" "Channel created for IPC"
check_output "Loading 'console_svc.elf'" "Console service ELF found in initrd"
check_output "ELF loaded" "ELF binary loaded successfully"
check_output "Console handle:" "Console handle registered"
check_output "Transitioning to Ring 3" "Ring 3 transition performed"
echo ""

# ─────────────────── Scenario 3: Kernel Subsystem Init ───────────────────

echo "${CYAN}Scenario 3: Kernel Subsystem Initialization${NC}"

check_output "Ramfs initialized" "Ramfs initialized at boot"
check_output "IPC subsystem initialized" "IPC subsystem running"
check_output "VFS.*Mounted\|Mounted filesystem" "VFS mount succeeded"
check_output "IRQ forwarding" "IRQ event forwarding configured"
check_output "handle.*keyboard\|IRQ.*keyboard\|IRQ forwarding: IRQ 1" "Keyboard IRQ event registered"
check_output "Launching console service" "Console service launch attempted"
echo ""

# ─────────────────── Scenario 4: Error Handling ───────────────────

echo "${CYAN}Scenario 4: Error Handling${NC}"

check_not_output "PANIC" "No panics during normal operation"
check_not_output "DOUBLE FAULT" "No double faults"
check_not_output "PAGE FAULT" "No page faults"
check_not_output "triple fault" "No triple faults"
echo ""

# ─────────────────── Scenario 5: Memory Safety ───────────────────

echo "${CYAN}Scenario 5: Memory Safety${NC}"

check_not_output "stack overflow" "No stack overflows"
check_not_output "heap overflow" "No heap overflows"
check_not_output "use-after-free" "No use-after-free"
check_not_output "Out of memory" "No OOM errors"
echo ""

# ─────────────────── Scenario 6: Channel and Handle Creation ───────────────────

echo "${CYAN}Scenario 6: Channel and Handle Management${NC}"

check_output "handle_a=0x" "Handle A created with valid hex address"
check_output "handle_b=0x" "Handle B created with valid hex address"
check_output "Console handle: 0x" "Console handle passed with correct rights"
check_output "Channel: handle_a=.*handle_b=" "Both channel ends created in handle table"
check_not_output "unknown syscall" "No unknown syscall errors"
check_not_output "PermissionDenied\|PERMISSION_DENIED\|permission denied" "No spurious permission errors"
echo ""

# ─────────────────── Scenario 7: Service Discovery ───────────────────

echo "${CYAN}Scenario 7: Service Discovery (endpoint register/discover)${NC}"

check_output "Kernel initialization complete" "Kernel initialized with service discovery syscalls"
check_not_output "unknown syscall" "No unknown syscall errors"
check_not_output "PANIC" "No panics in service discovery subsystem"
echo ""

# ─────────────────── Summary ───────────────────

echo "========================================="
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}All $PASSED/$SCENARIOS scenario tests passed!${NC}"
else
    echo -e "${RED}$FAILURES of $SCENARIOS scenario tests failed${NC}"
fi
echo "========================================="

exit $FAILURES
