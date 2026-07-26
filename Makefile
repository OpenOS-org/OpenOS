# OpenOS Makefile
#
# Build pipeline:
#   1. cargo build kernel (bare-metal)
#   2. nasm + ld: assemble user-space programs
#   3. mkinitrd.py: create initrd archive
#   4. cargo run disk image builder (sets ramdisk)

.PHONY: all build release run run-gui run-release debug clean lint fmt check help user initrd

KERNEL_ELF = target/x86_64-unknown-none/debug/openos-kernel
KERNEL_ELF_REL = target/x86_64-unknown-none/release/openos-kernel
BIOS_IMG = target/debug/openos-bios.img
BIOS_IMG_REL = target/release/openos-bios.img
INITRD = target/debug/initrd.img

# Kernel build flags (bare-metal, needs build-std)
KERNEL_TARGET = --target x86_64-unknown-none
BUILD_STD = -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# User-space programs
USER_SRC = user/hello.asm
USER_ELF = target/debug/hello.elf

# Default target
all: build

# Build user-space ELF programs
user:
	mkdir -p target/debug
	nasm -f elf64 user/hello.asm -o target/debug/hello.o
	ld -static -o $(USER_ELF) target/debug/hello.o
	@echo "Built $(USER_ELF)"

# Build initrd archive
initrd: user
	python3 tools/mkinitrd.py $(INITRD) hello.elf=$(USER_ELF)
	@echo "Built $(INITRD)"

# Build kernel + initrd + disk image (debug)
build: initrd
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD)
	cargo run -p openos --target x86_64-unknown-linux-gnu -- $(KERNEL_ELF) $(BIOS_IMG) $(INITRD)

# Build kernel + disk image (release)
release:
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) --release
	cargo run -p openos --target x86_64-unknown-linux-gnu --release -- $(KERNEL_ELF_REL) $(BIOS_IMG_REL) $(INITRD)

# Run clippy on kernel
lint:
	cargo clippy -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD) -- -D warnings

# Check formatting
fmt:
	cargo fmt --check

# Run all checks
check: fmt lint build

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
	rm -f target/debug/hello.o $(USER_ELF) $(INITRD)

help:
	@echo "OpenOS Build System (bootloader 0.11)"
	@echo "====================================="
	@echo "  make build       - Build kernel + initrd + BIOS disk image"
	@echo "  make release     - Build optimized"
	@echo "  make check       - fmt + clippy + build"
	@echo "  make lint        - Run clippy"
	@echo "  make fmt         - Check formatting"
	@echo "  make run         - QEMU (serial output)"
	@echo "  make run-gui     - QEMU (graphical)"
	@echo "  make run-release - QEMU optimized"
	@echo "  make debug       - QEMU + GDB"
	@echo "  make clean       - Clean artifacts"
	@echo "  make user        - Build user-space ELF programs"
	@echo "  make initrd      - Build initrd archive"
