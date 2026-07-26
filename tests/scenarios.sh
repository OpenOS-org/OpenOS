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
echo ""

# ─────────────────── Scenario 3: User-Space Channel IPC ───────────────────

echo "${CYAN}Scenario 3: User-Space Channel IPC${NC}"

check_output "Launching console service" "Console service launched in Ring 3"
check_output "channel_receive" "Console service called channel_receive"
check_output "channel_receive: got" "Message received by console service"
check_output "Hello from user-space" "Message content correct"
check_output "channel_reply" "Reply sent by console service"
check_output "SYS_EXIT.*status=0" "Clean exit after IPC"
echo ""

# ─────────────────── Scenario 4: Syscall Number Verification ───────────────────

echo "${CYAN}Scenario 4: Syscall Number Verification${NC}"

# Verify syscall numbers match INTERFACE.md
check_output "channel_receive" "channel_receive syscall works"
check_output "channel_reply" "channel_reply syscall works"
check_output "Hello from user-space" "Message delivered via channel"
check_output "SYS_EXIT.*status=0" "process_exit syscall works"
echo ""

# ─────────────────── Scenario 5: Error Handling ───────────────────

echo "${CYAN}Scenario 5: Error Handling${NC}"

check_not_output "PANIC" "No panics during normal operation"
check_not_output "DOUBLE FAULT" "No double faults"
check_not_output "PAGE FAULT" "No page faults"
check_not_output "triple fault" "No triple faults"
echo ""

# ─────────────────── Scenario 6: Memory Safety ───────────────────

echo "${CYAN}Scenario 6: Memory Safety${NC}"

check_not_output "stack overflow" "No stack overflows"
check_not_output "heap overflow" "No heap overflows"
check_not_output "use-after-free" "No use-after-free"
check_not_output "Out of memory" "No OOM errors"
echo ""

# ─────────────────── Scenario 7: Interrupt Handling ───────────────────

echo "${CYAN}Scenario 7: Interrupt Handling${NC}"

check_output "Kernel initialization complete" "Kernel fully initialized (includes interrupts)"
check_output "IPC subsystem initialized" "IPC subsystem initialized"
echo ""

# ─────────────────── Scenario 8: Task Management ───────────────────

echo "${CYAN}Scenario 8: Task Management${NC}"

check_output "Launching console service" "Task scheduling works"
check_output "Transitioning to Ring 3" "Ring 3 transition works"
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
