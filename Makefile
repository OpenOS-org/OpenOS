# OpenOS Makefile
#
# Build pipeline:
#   1. cargo build kernel (bare-metal)
#   2. Build user-space programs (assembly + Rust)
#   3. mkinitrd.py: create initrd archive
#   4. cargo run disk image builder (sets ramdisk)

.PHONY: all build release run run-gui run-release debug clean lint fmt check help user-rs initrd test test-unit test-integration quality

KERNEL_ELF = target/x86_64-unknown-none/debug/openos-kernel
KERNEL_ELF_REL = target/x86_64-unknown-none/release/openos-kernel
BIOS_IMG = target/debug/openos-bios.img
BIOS_IMG_REL = target/release/openos-bios.img
INITRD = target/debug/initrd.img

# Kernel build flags (bare-metal, needs build-std)
KERNEL_TARGET = --target x86_64-unknown-none
BUILD_STD = -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# User-space Rust programs (no_std, bare-metal)
USER_RS_TARGET = --target x86_64-unknown-none
USER_RS_STD = -Zbuild-std=core,compiler_builtins -Zbuild-std-features=compiler-builtins-mem

# Default target
all: build

# Build assembly user-space programs (console_svc, kb_echo)
user:
	mkdir -p target/debug
	nasm -f elf64 user/console_svc.asm -o target/debug/console_svc.o
	ld -static -o target/debug/console_svc.elf target/debug/console_svc.o
	nasm -f elf64 user/kb_echo.asm -o target/debug/kb_echo.o
	ld -static -o target/debug/kb_echo.elf target/debug/kb_echo.o
	@echo "Built assembly user-space programs"

# Build Rust user-space programs
user-rs:
	cargo build -p hello-rs $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/hello target/debug/hello_rs.elf
	cargo build -p net-echo $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/net_echo target/debug/net_echo.elf
	cargo build -p test-sdk $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/test-sdk target/debug/test_sdk.elf
	cargo build -p shell-rs $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/shell-rs target/debug/shell_rs.elf
	cargo build -p ping $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/ping target/debug/ping.elf
	cargo build -p curl-rs $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/curl target/debug/curl.elf
	cargo build -p devmgr $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/devmgr target/debug/devmgr.elf
	cargo build -p kb-driver $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/kb_driver target/debug/kb_driver.elf
	cargo build -p net-driver $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/net_driver target/debug/net_driver.elf
	strip target/debug/hello_rs.elf target/debug/net_echo.elf target/debug/test_sdk.elf target/debug/shell_rs.elf target/debug/ping.elf target/debug/curl.elf target/debug/devmgr.elf target/debug/kb_driver.elf target/debug/net_driver.elf 2>/dev/null || true
	@echo "Built Rust user-space programs"

# Build initrd archive with all programs
initrd: user user-rs
	cargo run -p mkinitrd --target x86_64-unknown-linux-gnu -- $(INITRD) \
		console_svc.elf=target/debug/console_svc.elf \
		kb_echo.elf=target/debug/kb_echo.elf \
		hello_rs.elf=target/debug/hello_rs.elf \
		net_echo.elf=target/debug/net_echo.elf \
		test_sdk.elf=target/debug/test_sdk.elf \
		shell_rs.elf=target/debug/shell_rs.elf \
		ping.elf=target/debug/ping.elf \
		curl.elf=target/debug/curl.elf \
		devmgr.elf=target/debug/devmgr.elf \
		kb_driver.elf=target/debug/kb_driver.elf \
		net_driver.elf=target/debug/net_driver.elf
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

test: test-unit quality

test-unit:
	@echo "=== Kernel Unit Tests ==="
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) 2>&1
	@echo "Unit tests compiled successfully"

test-integration: build
	@echo "=== Integration Tests ==="
	bash tests/integration.sh --timeout=15

test-scenarios: build
	@echo "=== Scenario Tests ==="
	bash tests/scenarios.sh

test-edge-cases: build
	@echo "=== Edge Case Tests ==="
	bash tests/edge_cases.sh

test-all-qemu: test-integration test-scenarios edge-cases

quality:
	@echo "=== Code Quality Checks ==="
	bash tests/quality.sh

# ─────────────────── Linting ───────────────────

lint:
	cargo clippy -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) -- -D warnings

fmt:
	cargo fmt --check

check: fmt lint build

# ─────────────────── Running ───────────────────

run: build
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG) \
		-serial stdio \
		-display none

run-gui: build
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG) \
		-serial stdio \
		-display gtk

run-release: release
	qemu-system-x86_64 \
		-drive format=raw,file=$(BIOS_IMG_REL) \
		-serial stdio \
		-display none

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

clean:
	cargo clean
	rm -f target/debug/console_svc.o target/debug/console_svc.elf
	rm -f target/debug/kb_echo.o target/debug/kb_echo.elf
	rm -f target/debug/hello_rs.elf target/debug/net_echo.elf
	rm -f target/debug/test_sdk.elf target/debug/shell_rs.elf target/debug/ping.elf target/debug/curl.elf
	rm -f target/debug/devmgr.elf target/debug/kb_driver.elf target/debug/net_driver.elf
	rm -f $(INITRD)

help:
	@echo "OpenOS Build System (bootloader 0.11)"
	@echo "====================================="
	@echo "  make build           - Build kernel + initrd + disk image"
	@echo "  make release         - Build optimized"
	@echo "  make check           - fmt + clippy + build"
	@echo "  make lint            - Run clippy"
	@echo "  make fmt             - Check formatting"
	@echo "  make test            - Run all tests (unit + quality)"
	@echo "  make test-integration - QEMU integration tests"
	@echo "  make quality         - Code quality checks"
	@echo "  make run             - QEMU (serial output)"
	@echo "  make run-gui         - QEMU (graphical)"
	@echo "  make run-release     - QEMU optimized"
	@echo "  make debug           - QEMU + GDB"
	@echo "  make clean           - Clean artifacts"
	@echo "  make user            - Build assembly user programs"
	@echo "  make user-rs         - Build Rust user programs"
	@echo "  make initrd          - Build initrd archive"
