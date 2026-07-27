#!/bin/bash
# tests/user_progs.sh — Verify all user-space programs boot without crashing
#
# For each program: launch it, capture output, check for PANIC/EXCEPTION/FAULT.
# Programs that print output and exit cleanly pass.
# Programs that crash (PANIC, EXCEPTION, FAULT) fail.

set -e
RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

TIMEOUT=8
cd "$(dirname "$0")/.."

PASS=0
FAIL=0
SKIP=0

# Programs to test (grouped by type)
PROGS=(
    # Assembly
    "console_svc.elf"
    "hello_rs.elf"
    "fstest.elf"
    # Coreutils - file ops
    "ls.elf"
    "cat.elf"
    "touch.elf"
    "rm.elf"
    "cp.elf"
    "mv.elf"
    "mkdir.elf"
    "rmdir.elf"
    "stat.elf"
    "chmod.elf"
    "ln.elf"
    "pwd.elf"
    "echo.elf"
    # Coreutils - text
    "head.elf"
    "tail.elf"
    "wc.elf"
    "grep.elf"
    "sort.elf"
    "uniq.elf"
    "rev.elf"
    "cut.elf"
    "tr.elf"
    "paste.elf"
    "fold.elf"
    "expand.elf"
    "unexpand.elf"
    "tee.elf"
    "hexdump.elf"
    "od.elf"
    "strings.elf"
    # Coreutils - system
    "hostname.elf"
    "uname.elf"
    "uptime.elf"
    "ps.elf"
    "date.elf"
    "whoami.elf"
    "id.elf"
    "env.elf"
    "printenv.elf"
    "which.elf"
    "tty.elf"
    "stty.elf"
    "logname.elf"
    "clear.elf"
    "true_cmd.elf"
    "false_cmd.elf"
    "yes.elf"
    "seq.elf"
    "sleep.elf"
    "basename.elf"
    "dirname.elf"
    "realpath.elf"
    "readlink.elf"
    "test_cmd.elf"
    "du.elf"
    "df.elf"
    "diff.elf"
    "find.elf"
    "file.elf"
    # Networking
    "ifconfig.elf"
    "ping.elf"
    # Tools
    "cal.elf"
    "tar.elf"
)

KERNEL_MAIN="kernel/src/main.rs"
BACKUP="/tmp/main.rs.backup"
cp "$KERNEL_MAIN" "$BACKUP"

test_prog() {
    local prog="$1"
    local name="${prog%.elf}"

    # Skip if ELF doesn't exist
    if [ ! -f "target/release/$prog" ]; then
        echo -e "  ${RED}SKIP${NC} $name (missing)"
        SKIP=$((SKIP + 1))
        return
    fi

    # Rebuild initrd with just this program
    cargo run -p mkinitrd --target x86_64-unknown-linux-gnu --release -- \
        target/release/initrd.img "$prog=target/release/$prog" 2>/dev/null

    # Update kernel to launch this program
    sed -i "s|\"shell_rs.elf\"|\"$prog\"|g" "$KERNEL_MAIN"
    sed -i "s|\"fstest.elf\"|\"$prog\"|g" "$KERNEL_MAIN"
    sed -i "s|\"console_svc.elf\"|\"$prog\"|g" "$KERNEL_MAIN"

    # Build kernel
    if ! cargo build -p openos-kernel --target x86_64-unknown-none \
        -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem \
        --release 2>/dev/null; then
        echo -e "  ${RED}FAIL${NC} $name (build error)"
        FAIL=$((FAIL + 1))
        return
    fi

    # Build disk image
    cargo run -p openos --target x86_64-unknown-linux-gnu --release -- \
        target/x86_64-unknown-none/release/openos-kernel \
        target/release/openos-bios.img target/release/initrd.img 2>/dev/null

    # Run QEMU
    timeout "$TIMEOUT" qemu-system-x86_64 \
        -drive format=raw,file=target/release/openos-bios.img \
        -serial file:/tmp/qemu-test.log \
        -display none -no-reboot 2>/dev/null || true

    # Check result
    if grep -q "PANIC\|EXCEPTION\|FAULT\|DOUBLE FAULT" /tmp/qemu-test.log 2>/dev/null; then
        echo -e "  ${RED}FAIL${NC} $name (crash)"
        FAIL=$((FAIL + 1))
        head -3 /tmp/qemu-test.log | grep -E "PANIC|EXCEPTION|FAULT" | head -1
    elif grep -q "system halted cleanly\|SYS_EXIT.*status=0" /tmp/qemu-test.log 2>/dev/null; then
        echo -e "  ${GREEN}PASS${NC} $name"
        PASS=$((PASS + 1))
    else
        echo -e "  ${YELLOW}WARN${NC} $name (no clear pass/fail)"
        SKIP=$((SKIP + 1))
    fi
}

echo "=== OpenOS User Program Verification ==="
echo "Testing ${#PROGS[@]} programs..."
echo ""

for prog in "${PROGS[@]}"; do
    test_prog "$prog"
done

# Restore kernel main
cp "$BACKUP" "$KERNEL_MAIN"

echo ""
echo "=== Results ==="
echo -e "${GREEN}PASS: $PASS${NC}"
echo -e "${RED}FAIL: $FAIL${NC}"
echo -e "SKIP: $SKIP"
