# OpenOS Makefile

.PHONY: all build run clean test lint fmt check

# Build flags for bare-metal kernel
BUILD_FLAGS = -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# Default target
all: build

# Build the kernel
build:
	cargo build $(BUILD_FLAGS)

# Build in release mode
release:
	cargo build --release $(BUILD_FLAGS)

# Run clippy linter
lint:
	cargo clippy $(BUILD_FLAGS) -- -D warnings

# Check formatting
fmt:
	cargo fmt --check

# Run all checks (format + lint + build)
check: fmt lint build

# Run in QEMU
run: build
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-openos.bin \
		-serial stdio \
		-display gtk

# Run in QEMU (release mode)
run-release: release
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/release/bootimage-openos.bin \
		-serial stdio \
		-display gtk

# Run with serial output only
run-serial: build
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-openos.bin \
		-serial stdio \
		-nographic

# Clean build artifacts
clean:
	cargo clean

# Run tests
test:
	cargo test $(BUILD_FLAGS)

# Create bootable ISO
iso: build
	bootimage build $(BUILD_FLAGS)

# Run with bootimage
bootimage: build
	bootimage run $(BUILD_FLAGS)

# Debug with GDB
debug: build
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-openos.bin \
		-s -S &
	sleep 1
	gdb-multiarch \
		-ex "target remote :1234" \
		-ex "set architecture i386:x86-64" \
		-ex "break _start" \
		-ex "continue"

# Show help
help:
	@echo "OpenOS Build System"
	@echo "==================="
	@echo "  make build       - Build the kernel"
	@echo "  make release     - Build in release mode"
	@echo "  make lint        - Run clippy linter"
	@echo "  make fmt         - Check formatting"
	@echo "  make check       - Run all checks (fmt + lint + build)"
	@echo "  make run         - Run in QEMU"
	@echo "  make run-release - Run in QEMU (release)"
	@echo "  make run-serial  - Run with serial output"
	@echo "  make clean       - Clean build artifacts"
	@echo "  make test        - Run tests"
	@echo "  make iso         - Create bootable ISO"
	@echo "  make bootimage   - Run with bootimage"
	@echo "  make debug       - Debug with GDB"
	@echo "  make help        - Show this help"
