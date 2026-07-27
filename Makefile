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
UEFI_IMG = target/debug/openos-uefi.img
INITRD = target/debug/initrd.img

# Kernel build flags (bare-metal, needs build-std)
KERNEL_TARGET = --target x86_64-unknown-none
BUILD_STD = -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# User-space Rust programs (no_std, bare-metal)
USER_RS_TARGET = --target x86_64-unknown-none
USER_RS_STD = -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

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
	cargo build -p ld-so $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/ld_so target/debug/ld_so.elf
	cargo build -p nc-rs $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/nc target/debug/nc.elf
	cargo build -p ifconfig-rs $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/ifconfig target/debug/ifconfig.elf
	cargo build -p cal $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/cal target/debug/cal.elf
	cargo build -p man $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/man target/debug/man.elf
	cargo build -p tar $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/tar target/debug/tar.elf
	cargo build -p pthreads $(USER_RS_TARGET) $(USER_RS_STD)
	cargo build -p test-pthreads $(USER_RS_TARGET) $(USER_RS_STD)
	cp target/x86_64-unknown-none/debug/test_pthreads target/debug/test_pthreads.elf
	cargo build -p coreutils $(USER_RS_TARGET) $(USER_RS_STD)
	@# Copy all coreutils binaries
	@for cmd in ls cat echo pwd touch rm cp mv head tail wc grep sort uniq rev tee hexdump hostname uname uptime ps date sleep yes seq true_cmd false_cmd basename dirname id whoami clear env which du df chmod ln mkdir rmdir find diff cut tr paste fold expand unexpand od strings file stat realpath readlink test_cmd printenv logname tty stty; do \
		cp target/x86_64-unknown-none/debug/$$cmd target/debug/$$cmd.elf 2>/dev/null || true; \
	done
	strip target/debug/hello_rs.elf target/debug/net_echo.elf target/debug/test_sdk.elf target/debug/shell_rs.elf target/debug/ping.elf target/debug/curl.elf target/debug/devmgr.elf target/debug/kb_driver.elf target/debug/net_driver.elf target/debug/ld_so.elf target/debug/nc.elf target/debug/ifconfig.elf target/debug/cal.elf target/debug/man.elf target/debug/tar.elf target/debug/test_pthreads.elf 2>/dev/null || true
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
		net_driver.elf=target/debug/net_driver.elf \
		ld_so.elf=target/debug/ld_so.elf \
		ls.elf=target/debug/ls.elf \
		cat.elf=target/debug/cat.elf \
		echo.elf=target/debug/echo.elf \
		pwd.elf=target/debug/pwd.elf \
		touch.elf=target/debug/touch.elf \
		rm.elf=target/debug/rm.elf \
		cp.elf=target/debug/cp.elf \
		mv.elf=target/debug/mv.elf \
		head.elf=target/debug/head.elf \
		tail.elf=target/debug/tail.elf \
		wc.elf=target/debug/wc.elf \
		grep.elf=target/debug/grep.elf \
		sort.elf=target/debug/sort.elf \
		uniq.elf=target/debug/uniq.elf \
		rev.elf=target/debug/rev.elf \
		tee.elf=target/debug/tee.elf \
		hexdump.elf=target/debug/hexdump.elf \
		hostname.elf=target/debug/hostname.elf \
		uname.elf=target/debug/uname.elf \
		uptime.elf=target/debug/uptime.elf \
		ps.elf=target/debug/ps.elf \
		date.elf=target/debug/date.elf \
		sleep.elf=target/debug/sleep.elf \
		yes.elf=target/debug/yes.elf \
		seq.elf=target/debug/seq.elf \
		basename.elf=target/debug/basename.elf \
		dirname.elf=target/debug/dirname.elf \
		id.elf=target/debug/id.elf \
		whoami.elf=target/debug/whoami.elf \
		clear.elf=target/debug/clear.elf \
		env.elf=target/debug/env.elf \
		which.elf=target/debug/which.elf \
		du.elf=target/debug/du.elf \
		df.elf=target/debug/df.elf \
		chmod.elf=target/debug/chmod.elf \
		ln.elf=target/debug/ln.elf \
		mkdir.elf=target/debug/mkdir.elf \
		rmdir.elf=target/debug/rmdir.elf \
		find.elf=target/debug/find.elf \
		diff.elf=target/debug/diff.elf \
		cut.elf=target/debug/cut.elf \
		tr.elf=target/debug/tr.elf \
		paste.elf=target/debug/paste.elf \
		fold.elf=target/debug/fold.elf \
		expand.elf=target/debug/expand.elf \
		unexpand.elf=target/debug/unexpand.elf \
		od.elf=target/debug/od.elf \
		strings.elf=target/debug/strings.elf \
		file.elf=target/debug/file.elf \
		stat.elf=target/debug/stat.elf \
		realpath.elf=target/debug/realpath.elf \
		readlink.elf=target/debug/readlink.elf \
		test.elf=target/debug/test_cmd.elf \
		printenv.elf=target/debug/printenv.elf \
		logname.elf=target/debug/logname.elf \
		tty.elf=target/debug/tty.elf \
		stty.elf=target/debug/stty.elf \
		true.elf=target/debug/true_cmd.elf \
		false.elf=target/debug/false_cmd.elf \
		ifconfig.elf=target/debug/ifconfig.elf \
		nc.elf=target/debug/nc.elf \
		cal.elf=target/debug/cal.elf \
		man.elf=target/debug/man.elf \
		tar.elf=target/debug/tar.elf \
		test_pthreads.elf=target/debug/test_pthreads.elf
	@echo "Built $(INITRD)"

# Build kernel + initrd + disk image (debug)
build: initrd
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD)
	cargo run -p openos --target x86_64-unknown-linux-gnu -- $(KERNEL_ELF) $(BIOS_IMG) $(INITRD)

# Build kernel + UEFI disk image (debug)
build-uefi: initrd
	cargo build -p openos-kernel $(KERNEL_TARGET) $(BUILD_STD)
	cargo run -p openos --target x86_64-unknown-linux-gnu -- $(KERNEL_ELF) $(UEFI_IMG) $(INITRD) --uefi

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

run-uefi: build-uefi
	qemu-system-x86_64 \
		-drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE.fd \
		-drive if=pflash,format=raw,file=/usr/share/OVMF/OVMF_VARS.fd \
		-drive format=raw,file=$(UEFI_IMG) \
		-serial stdio \
		-display none

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
	rm -f target/debug/test_sdk.elf target/debug/shell_rs.elf target/debug/ping.elf target/debug/curl.elf target/debug/cal.elf target/debug/man.elf target/debug/tar.elf target/debug/test_pthreads.elf
	rm -f target/debug/devmgr.elf target/debug/kb_driver.elf target/debug/net_driver.elf
	rm -f $(INITRD)

help:
	@echo "OpenOS Build System (bootloader 0.11)"
	@echo "====================================="
	@echo "  make build           - Build kernel + initrd + BIOS disk image"
	@echo "  make build-uefi      - Build kernel + initrd + UEFI disk image"
	@echo "  make release         - Build optimized"
	@echo "  make check           - fmt + clippy + build"
	@echo "  make lint            - Run clippy"
	@echo "  make fmt             - Check formatting"
	@echo "  make test            - Run all tests (unit + quality)"
	@echo "  make test-integration - QEMU integration tests"
	@echo "  make quality         - Code quality checks"
	@echo "  make run             - QEMU BIOS (serial output)"
	@echo "  make run-uefi        - QEMU UEFI (serial output)"
	@echo "  make run-gui         - QEMU (graphical)"
	@echo "  make run-release     - QEMU optimized"
	@echo "  make debug           - QEMU + GDB"
	@echo "  make clean           - Clean artifacts"
	@echo "  make user            - Build assembly user programs"
	@echo "  make user-rs         - Build Rust user programs"
	@echo "  make initrd          - Build initrd archive"
