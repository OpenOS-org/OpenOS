# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OpenOS is a bare-metal microkernel operating system written in Rust, targeting x86_64. It runs directly on hardware (or QEMU) with no underlying OS — `#![no_std]` and `#![no_main]`.

## Build & Development Commands

This is a bare-metal kernel, so normal `cargo build` won't work. All build commands require nightly features and the `build-std` flag. Use the Makefile:

```bash
make build       # Build the kernel
make release     # Build optimized
make check       # Run all checks (fmt + clippy + build) — use this before committing
make lint        # Run clippy with -D warnings
make fmt         # Check formatting (cargo fmt --check)
make run         # Build and run in QEMU with GTK display
make run-serial  # Build and run in QEMU with serial output only (no GUI)
make debug       # Run in QEMU with GDB stub, then attach gdb-multiarch
make clean       # Clean build artifacts
```

The raw cargo equivalent for any command requires these flags:
```
cargo <cmd> -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem
```

## Architecture

### Microkernel Design

The kernel follows a microkernel architecture where only essential services run in kernel space:
- **Memory management** (`memory/`) — heap allocator, frame allocation
- **Task scheduling** (`task/`) — round-robin scheduler, task control blocks
- **IPC** (`ipc/`) — message passing with ports (BTreeMap-based port registry)
- **System calls** (`syscall/`) — dispatcher for user-space → kernel transitions

Everything else (drivers, filesystem, network) is designed to eventually run in user space.

### Architecture Layer (`arch/x86_64/`)

- `gdt.rs` — GDT + TSS setup (double fault IST stack)
- `interrupts.rs` — IDT, PIC 8259 initialization, hardware interrupt handlers (timer, keyboard)
- `linker.ld` — Higher-half kernel linker script (kernel mapped at `0xFFFFFFFF80100000`)

### Boot Sequence (`main.rs:_start`)

1. VGA init → 2. GDT/IDT/PIC → 3. Memory (heap) → 4. Syscall handler → 5. IPC → 6. Task scheduler → 7. Idle loop (`hlt`)

### Output

- **VGA text buffer** (`drivers/vga.rs`) — 80×25 color text at `0xB8000`, provides `print!`/`println!` macros
- **Serial** (`drivers/serial.rs`) — UART 16550 at `0x3F8`, provides `serial_print!`/`serial_println!` macros (visible in QEMU with `-serial stdio`)

## Key Dependencies

- `bootloader 0.9` — BIOS bootloader, loads kernel into memory
- `x86_64 0.15` — CPU structures (GDT, IDT, paging, port I/O)
- `pic8259` — PIC initialization
- `uart_16550` — Serial port driver
- `spin` — Spinlock (used everywhere for `Mutex`)
- `linked_list_allocator` — Kernel heap allocator

## Lint Configuration

Strict clippy is enabled in `main.rs`:
```rust
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
```

`dead_code` and `unused_variables` are allowed at the crate level since much of the scaffolding is not yet wired up. Formatting uses `rustfmt.toml` with `group_imports = "StdExternalCrate"`.

## Build Target

Uses the built-in `x86_64-unknown-none` target (no OS, no SSE for kernel code, panic=abort). The custom linker script is passed via `.cargo/config.toml` rustflags.

## Skills (Slash Commands)

Skills in `.claude/skills/` encapsulate common development workflows:

| Skill | Purpose |
|-------|---------|
| `/kernel-check` | Run full CI pipeline (fmt → clippy → build), fix all issues |
| `/kernel-run [mode]` | Build and launch in QEMU (`gui`, `serial`, `release`) |
| `/kernel-debug` | Launch QEMU with GDB stub for step debugging |
| `/add-driver <name>` | Scaffold a new device driver with IRQ handler |
| `/add-interrupt <irq> <name>` | Register a new hardware interrupt handler in IDT |
| `/add-syscall <name> <num>` | Add a new system call to the dispatcher |
| `/add-module <name>` | Scaffold a new kernel subsystem module |
| `/add-ipc-service <name>` | Create an IPC service with request/response protocol |
| `/fix-panic [context]` | Diagnose and fix kernel panic or triple fault |

## Documentation

- `README.md` — Project overview, quick start, architecture (bilingual EN/CN)
- `docs/ADR.md` — Architecture Decision Records (10 decisions documented)
- `CLAUDE.md` — This file, development guidance for Claude Code
