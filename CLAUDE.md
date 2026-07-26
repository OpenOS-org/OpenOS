# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OpenOS is a bare-metal microkernel operating system written in Rust, targeting x86_64. It runs directly on hardware (or QEMU) with no underlying OS — `#![no_std]` and `#![no_main]`.

## Build & Development Commands

The build system uses a two-step process: compile the kernel, then create a bootable disk image.

```bash
make build       # Build kernel + BIOS disk image
make release     # Build optimized
make check       # Run all checks (fmt + clippy + build) — use this before committing
make lint        # Run clippy with -D warnings
make fmt         # Check formatting (cargo fmt --check)
make run         # Build and run in QEMU (serial output)
make run-gui     # Build and run in QEMU (graphical display)
make debug       # Run in QEMU with GDB stub
make clean       # Clean build artifacts
```

Raw cargo equivalents:
```bash
# Build kernel (bare-metal)
cargo build -p openos-kernel --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# Build user-space program
nasm -f elf64 user/hello.asm -o target/debug/hello.o
ld -static -o target/debug/hello.elf target/debug/hello.o

# Create initrd
python3 tools/mkinitrd.py target/debug/initrd.img hello.elf=target/debug/hello.elf

# Create disk image (with ramdisk)
cargo run -p openos --target x86_64-unknown-linux-gnu -- \
  target/x86_64-unknown-none/debug/openos-kernel target/debug/openos-bios.img target/debug/initrd.img
```

## Architecture

### Workspace Layout

```
openos/
  Cargo.toml          # Workspace root + disk image builder crate
  src/main.rs         # Disk image builder (bootloader::BiosBoot + set_ramdisk)
  kernel/
    Cargo.toml        # Kernel crate (depends on bootloader_api)
    src/main.rs       # entry_point!(kernel_main) — kernel entry
    src/elf.rs        # Minimal ELF64 parser and loader
    src/initrd.rs     # Initrd archive parser (custom binary format)
    src/frame_alloc.rs # Bump frame allocator for user pages
    src/arch/         # GDT, IDT, PIC, SYSCALL
    src/drivers/      # Serial (UART 16550), framebuffer text renderer
    src/memory/       # Heap allocator (linked_list_allocator)
    src/syscall/      # System call dispatcher
    src/task/         # Scheduler, task management, user-mode (ELF loader)
    src/ipc/          # IPC message passing
    src/fs/           # VFS placeholder
  user/
    hello.asm         # User-space test program (NASM)
  tools/
    mkinitrd.py       # Initrd archive builder
```

### Bootloader 0.11

The kernel uses `bootloader_api` 0.11 for boot. Key differences from 0.9:

- **Entry point**: `entry_point!(kernel_main)` macro replaces `fn _start() -> !`
- **BootInfo**: Provides framebuffer, memory map, physical memory offset
- **Framebuffer**: Replaces VGA text buffer at 0xB8000 — text rendered via pixel font
- **Disk images**: Created by `bootloader::BiosBoot` in the top-level crate
- **PIE kernel**: Compiled as position-independent executable for dynamic relocation
- **No custom linker script**: The bootloader handles page table setup

### ELF Loader + Initrd

User-space programs are loaded dynamically from an initrd (ramdisk):

1. **Build**: `user/hello.asm` → nasm → ld → `hello.elf`
2. **Archive**: `tools/mkinitrd.py` packs ELF binaries into a custom binary format
3. **Boot**: bootloader loads ramdisk into memory, provides `ramdisk_addr` in BootInfo
4. **Load**: kernel parses initrd, finds ELF by name, loads PT_LOAD segments
5. **Execute**: IRETQ to Ring 3 at the ELF entry point

The initrd format is a simple binary archive (magic `OSRD`, file table + data).

### Microkernel Design

The kernel follows a microkernel architecture where only essential services run in kernel space:
- **Memory management** (`memory/`) — heap allocator, `frame_alloc` bump allocator
- **Task scheduling** (`task/`) — round-robin scheduler, task control blocks
- **ELF loading** (`elf/`) — minimal ELF64 parser, PT_LOAD segment loader
- **Initrd** (`initrd/`) — ramdisk archive parser
- **IPC** (`ipc/`) — message passing with ports (BTreeMap-based port registry)
- **System calls** (`syscall/`) — dispatcher for user-space → kernel transitions

Everything else (drivers, filesystem, network) is designed to eventually run in user space.

### Architecture Layer (`arch/x86_64/`)

- `gdt.rs` — GDT + TSS setup (user segments for Ring 3, double fault IST stack)
- `interrupts.rs` — IDT, PIC 8259 initialization, hardware interrupt handlers
- `syscall.rs` — SYSCALL/SYSRET MSR configuration, syscall entry stub

### GDT Layout (SYSCALL/SYSRET)

```
Index 0: null (0x00)
Index 1: kernel code (0x08, Ring 0)
Index 2: kernel data (0x10, Ring 0)
Index 3: user data (0x18, Ring 3)  — must be before user code for SYSRET
Index 4: user code (0x20, Ring 3)
Index 5-6: TSS
```

### Output

- **Framebuffer** (`drivers/vga.rs`) — pixel-based text renderer with 8×16 bitmap font
- **Serial** (`drivers/serial.rs`) — UART 16550 at 0x3F8, provides `serial_print!`/`serial_println!`

## Key Dependencies

- `bootloader_api 0.11` — kernel boot interface (BootInfo, entry_point!)
- `bootloader 0.11` — disk image builder (BiosBoot)
- `x86_64 0.15` — CPU structures (GDT, IDT, paging, port I/O)
- `pic8259 0.11` — PIC initialization
- `uart_16550 0.3` — Serial port driver
- `spin 0.9` — Spinlock (used everywhere for `Mutex`)
- `linked_list_allocator 0.10` — Kernel heap allocator

## Lint Configuration

Strict clippy is enabled:
```rust
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
```

Formatting uses `rustfmt.toml` with `group_imports = "StdExternalCrate"`.

## Build Target

Uses `x86_64-unknown-none` target (no OS, panic=abort). The kernel is compiled as PIE for bootloader 0.11's dynamic relocation.

## Known Limitations

- **User-mode process**: Deferred — page table walk needs `physical_memory_offset` from BootInfo
- **Physical frame allocator**: Not implemented (returns None)
- **Keyboard input**: Scancode read but not decoded
