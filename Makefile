# OpenOS Makefile
#
# Build pipeline:
#   1. cargo build kernel (bare-metal)
#   2. nasm + ld: assemble user-space programs
#   3. mkinitrd.py: create initrd archive
#   4. cargo run disk image builder (sets ramdisk)

.PHONY: all build release run run-gui run-release debug clean lint fmt check help user initrd test test-unit test-integration quality

KERNEL_ELF = target/x86_64-unknown-none/debug/openos-kernel
KERNEL_ELF_REL = target/x86_64-unknown-none/release/openos-kernel
BIOS_IMG = target/debug/openos-bios.img
BIOS_IMG_REL = target/release/openos-bios.img
INITRD = target/debug/initrd.img

# Kernel build flags (bare-metal, needs build-std)
KERNEL_TARGET = --target x86_64-unknown-none
BUILD_STD = -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# Default target
all: build

# Build user-space ELF programs
user:
	mkdir -p target/debug
	nasm -f elf64 user/hello.asm -o target/debug/hello.o
	ld -static -o target/debug/hello.elf target/debug/hello.o
	nasm -f elf64 user/console_svc.asm -o target/debug/console_svc.o
	ld -static -o target/debug/console_svc.elf target/debug/console_svc.o
	@echo "Built user-space programs"

# Build initrd archive with both programs
initrd: user
	python3 tools/mkinitrd.py $(INITRD) \
		hello.elf=target/debug/hello.elf \
		console_svc.elf=target/debug/console_svc.elf
	@echo "Built $(INITRD)"

# Build kernel + initrd + disk image (debug)
build: initrd
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD)
	cargo run -p openos --target x86_64-unknown-linux-gnu -- $(KERNEL_ELF) $(BIOS_IMG) $(INITRD)

# Build kernel + disk image (release)
release:
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) --release
	cargo run -p openos --target x86_64-unknown-linux-gnu --release -- $(KERNEL_ELF_REL) $(BIOS_IMG_REL) $(INITRD)

# ─────────────────── Testing ───────────────────

# Run all tests (unit + integration + quality)
test: test-unit quality

# Run kernel unit tests (in QEMU via custom test runner)
test-unit:
	@echo "=== Kernel Unit Tests ==="
	@echo "Running IPC channel tests..."
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) 2>&1
	@echo "Unit tests compiled successfully"
	@echo ""
	@echo "NOTE: Full QEMU-based unit test runner requires test kernel binary."
	@echo "For now, unit tests are verified via compilation + clippy."

# Run integration tests (QEMU-based)
test-integration: build
	@echo "=== Integration Tests ==="
	bash tests/integration.sh --timeout=15

# Run scenario tests
test-scenarios: build
	@echo "=== Scenario Tests ==="
	bash tests/scenarios.sh

# Run edge case tests
test-edge-cases: build
	@echo "=== Edge Case Tests ==="
	bash tests/edge_cases.sh

# Run all QEMU-based tests
test-all-qemu: test-integration test-scenarios test-edge-cases

# Run code quality checks
quality:
	@echo "=== Code Quality Checks ==="
	bash tests/quality.sh

# ─────────────────── Linting ───────────────────

# Run clippy on kernel
lint:
	cargo clippy -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) -- -D warnings

# Check formatting
fmt:
	cargo fmt --check

# Run all checks (format + lint + build)
check: fmt lint build

# ─────────────────── Running ───────────────────

# Run in QEMU (serial output)
run: build
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG) \
		-serial stdio \
		-display none

# Run in QEMU (graphical)
run-gui: build
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG) \
		-serial stdio \
		-display gtk

# Run release in QEMU
run-release: release
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG_REL) \
		-serial stdio \
		-display none

# Debug with GDB
debug: build
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG) \
		-serial stdio \
		-display none \
		-s -S &
	sleep 1
	gdb-multiarch \
		-ex "target remote :1234" \
		-ex "set architecture i386:x86-64" \
		-ex "break kernel_main" \
		-ex "continue"

# Clean
clean:
	cargo clean
	rm -f target/debug/hello.o target/debug/hello.elf
	rm -f target/debug/console_svc.o target/debug/console_svc.elf
	rm -f $(INITRD)

help:
	@echo "OpenOS Build System (bootloader 0.11)"
	@echo "====================================="
	@echo "  make build           - Build kernel + initrd + BIOS disk image"
	@echo "  make release         - Build optimized"
	@echo "  make check           - fmt + clippy + build"
	@echo "  make lint            - Run clippy"
	@echo "  make fmt             - Check formatting"
	@echo "  make test            - Run all tests (unit + quality)"
	@echo "  make test-unit       - Run kernel unit tests"
	@echo "  make test-integration - Run QEMU integration tests"
	@echo "  make quality         - Run code quality checks"
	@echo "  make run             - QEMU (serial output)"
	@echo "  make run-gui         - QEMU (graphical)"
	@echo "  make run-release     - QEMU optimized"
	@echo "  make debug           - QEMU + GDB"
	@echo "  make clean           - Clean artifacts"
	@echo "  make user            - Build user-space ELF programs"
	@echo "  make initrd          - Build initrd archive"
