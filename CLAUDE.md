# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OpenOS is a bare-metal microkernel operating system written in Rust, targeting x86_64. It runs directly on hardware (or QEMU) with no underlying OS — `#![no_std]` and `#![no_main]`.

## Build & Development Commands

```bash
make build       # Build kernel + BIOS disk image
make build-uefi  # Build kernel + UEFI disk image
make release     # Build optimized
make check       # Run all checks (fmt + clippy + build) — use this before committing
make lint        # Run clippy with -D warnings
make fmt         # Check formatting (cargo fmt --check)
make run         # Build and run in QEMU (serial output)
make run-gui     # Build and run in QEMU (graphical display)
make run-uefi    # Build and run in QEMU with UEFI (OVMF)
make debug       # Run in QEMU with GDB stub
make clean       # Clean build artifacts
make user        # Build assembly user-space programs
make user-rs     # Build Rust user-space programs
make initrd      # Build initrd archive
make test        # Run unit tests + quality checks
```

Raw cargo equivalents:
```bash
# Build kernel (bare-metal)
cargo build -p openos-kernel --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# Run kernel tests (host target)
cargo test -p openos-kernel --target x86_64-unknown-linux-gnu

# Build user-space program (example)
cargo build -p hello-rs --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins -Zbuild-std-features=compiler-builtins-mem

# Create initrd
cargo run -p mkinitrd --target x86_64-unknown-linux-gnu -- target/debug/initrd.img \
  hello.elf=target/debug/hello.elf

# Create disk image (with ramdisk)
cargo run -p openos --target x86_64-unknown-linux-gnu -- \
  target/x86_64-unknown-none/debug/openos-kernel target/debug/openos-bios.img target/debug/initrd.img

# Create UEFI disk image
cargo run -p openos --target x86_64-unknown-linux-gnu -- \
  target/x86_64-unknown-none/debug/openos-kernel target/debug/openos-uefi.img target/debug/initrd.img --uefi
```

## Architecture

### Workspace Layout

```
openos/
  Cargo.toml          # Workspace root + disk image builder crate
  src/main.rs         # Disk image builder (BiosBoot + UefiBoot)
  kernel/
    Cargo.toml        # Kernel crate (depends on bootloader_api)
    src/lib.rs        # Module root (cfg-gated no_std/no_main for test support)
    src/main.rs       # entry_point!(kernel_main) — kernel entry
    src/elf.rs        # ELF64 parser, loader, PT_DYNAMIC/DT_* support
    src/initrd.rs     # Initrd archive parser (custom binary format)
    src/frame_alloc.rs # Bitmap frame allocator (4 KiB frames)
    src/handle.rs     # Capability-based Handle system (28/10/26 bit layout)
    src/sync.rs       # Interrupt-safe mutex (IntMutex)
    src/arch/         # GDT, IDT, PIC, SYSCALL, ACPI, APIC, SMP, per-CPU
    src/drivers/      # Serial, VGA, keyboard, PCI, VirtIO-Block, VirtIO-Net
    src/memory/       # Heap allocator, page table, VMA, DMA
    src/syscall/      # System call dispatcher (39 syscalls)
    src/task/         # Scheduler (SMP round-robin), task management, user-mode
    src/ipc/          # IPC channel message passing
    src/fs/           # VFS, ramfs, ext2, block cache
    src/net/          # Ethernet, ARP, IPv4, TCP, UDP, DHCP, DNS, sockets
  sdk/                # Rust user-space SDK
  user/               # User-space programs (11 binaries)
    hello_rs/         # Hello world test
    shell_rs/         # Interactive shell
    test_sdk/         # SDK integration test
    curl_rs/          # HTTP client
    ping/             # ICMP ping
    net_echo/         # Network echo server
    devmgr/           # Device manager
    kb_driver/        # User-space keyboard driver
    net_driver/       # User-space VirtIO-Net driver
    ld_so/            # Dynamic linker (ld.so)
    hello.asm         # Assembly hello (NASM)
    console_svc.asm   # Assembly console service (NASM)
    kb_echo.asm       # Assembly keyboard echo (NASM)
  tools/
    mkinitrd/         # Initrd archive builder (Rust)
```

### Boot Sequence

```
BIOS/UEFI → bootloader (0.11) → kernel_main(boot_info)
  │
  ├─ 1. Serial init         (drivers/serial.rs)
  ├─ 2. phys_offset store   (memory/mod.rs)       — physical_memory_offset from BootInfo
  ├─ 3. VGA/Framebuffer     (drivers/vga.rs)       — pixel font text renderer
  ├─ 4. GDT + IDT + PIC    (arch/x86_64/)         — segment descriptors, ISR handlers
  ├─ 5. SYSCALL MSRs        (arch/x86_64/syscall.rs) — STAR/LSTAR/SFMASK/EFER
  ├─ 6. Heap allocator      (memory/allocator.rs)  — 2 MiB linked_list_allocator
  ├─ 7. Frame allocator     (frame_alloc.rs)       — bitmap allocator (32-64 MiB)
  ├─ 8. VFS + ramfs         (fs/)                  — mount ramfs at "/"
  ├─ 9. VirtIO-Block        (drivers/virtio_block.rs) — PCI discovery, mount ext2 at "/disk"
  ├─ 10. Network            (net/)                 — VirtIO-Net, DHCP
  ├─ 11. IPC subsystem      (ipc/mod.rs)
  ├─ 12. Per-CPU data       (arch/x86_64/percpu.rs) — GSBASE for CPU 0
  ├─ 13. Scheduler          (task/scheduler.rs)    — SMP round-robin, idle task
  ├─ 14. IRQ forwarding     (handle.rs)            — keyboard IRQ 1 → IrqEvent
  └─ 15. Load first process (task/user.rs)         — ELF from initrd, IRETQ to Ring 3
```

### Microkernel Design

The kernel follows a microkernel architecture where only essential services run in kernel space:
- **Memory management** (`memory/`) — heap allocator, bitmap frame allocator, page table abstraction, VMA tracker, DMA
- **Task scheduling** (`task/`) — SMP round-robin scheduler with per-CPU queues, work stealing, IPI wakeup
- **ELF loading** (`elf/`) — ELF64 parser, PT_LOAD/PT_DYNAMIC segment loader, relocation support
- **Initrd** (`initrd/`) — ramdisk archive parser
- **IPC** (`ipc/`) — synchronous channel message passing with handle transfer
- **Capability security** (`handle.rs`) — Handle with 28-bit slot / 10-bit rights / 26-bit generation
- **System calls** (`syscall/`) — 39 syscalls across IPC, process, memory, filesystem, network, device
- **Networking** (`net/`) — full TCP/IP stack: Ethernet, ARP, IPv4, TCP, UDP, DHCP, DNS, BSD sockets
- **Filesystem** (`fs/`) — VFS abstraction, ramfs, ext2 (read/write), block cache
- **Drivers** (`drivers/`) — VirtIO-Block, VirtIO-Net, UART serial, PS/2 keyboard, PCI, framebuffer

### Architecture Layer (`arch/x86_64/`)

- `gdt.rs` — GDT + TSS setup (user segments for Ring 3, double fault IST stack)
- `interrupts.rs` — IDT, PIC 8259, exception handlers (user-fault-tolerant), timer preemption
- `syscall.rs` — SYSCALL/SYSRET MSR config, naked entry stub with CR3 switching
- `acpi.rs` — RSDP/RSDT/XSDT/MADT parsing (BIOS + UEFI RSDP)
- `apic.rs` — Local APIC + I/O APIC drivers, IPI, timer calibration
- `ap_start.rs` — AP boot via INIT+SIPI IPI, real→protected→long mode trampoline
- `percpu.rs` — Per-CPU data via GSBASE MSR (up to 8 CPUs)

### GDT Layout (SYSCALL/SYSRET)

```
Index 0: null (0x00)
Index 1: kernel code (0x08, Ring 0)
Index 2: kernel data (0x10, Ring 0)
Index 3: user data (0x18, Ring 3)  — must be before user code for SYSRET
Index 4: user code (0x20, Ring 3)
Index 5-6: TSS
```

### SavedContext Layout (136 bytes, 17 × u64)

```
Offset  Field       Purpose
0       r9          arg5
8       r8          arg4
16      rdx         arg3
24      rsi         arg2
32      rdi         arg1
40      rax         syscall number / return
48      r15         callee-saved
56      r14         callee-saved
64      r13         callee-saved
72      r12         callee-saved
80      rbx         callee-saved
88      rbp         callee-saved
96      r11         RFLAGS (from SYSCALL)
104     rcx         RIP (from SYSCALL)
112     rsp         stack pointer
120     is_kernel   1=kernel (IRETQ), 0=user (SYSRET)
128     cr3         page table physical address
```

### Handle Bit Layout (64-bit)

```
Bits 0-27:   slot_id    (28 bits, max 268M handles)
Bits 28-37:  rights     (10 bits, 10 permission flags)
Bits 38-63:  generation (26 bits, prevents use-after-close)
```

### Output

- **Framebuffer** (`drivers/vga.rs`) — pixel-based text renderer with 8×16 bitmap font
- **Serial** (`drivers/serial.rs`) — UART 16550 at 0x3F8, provides `serial_print!`/`serial_println!`

## Key Dependencies

- `bootloader_api 0.11` — kernel boot interface (BootInfo, entry_point!)
- `bootloader 0.11` — disk image builder (BiosBoot + UefiBoot)
- `x86_64 0.15` — CPU structures (GDT, IDT, paging, port I/O)
- `pic8259 0.11` — PIC initialization
- `uart_16550 0.3` — Serial port driver
- `spin 0.9` — Spinlock (used everywhere for `Mutex`)
- `linked_list_allocator 0.10` — Kernel heap allocator
- `pc-keyboard 0.7` — PS/2 keyboard scancode decoding

## Lint Configuration

Strict clippy is enabled:
```rust
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
```

Formatting uses `rustfmt.toml` with `group_imports = "StdExternalCrate"`.

## Build Target

Uses `x86_64-unknown-none` target (no OS, panic=abort). The kernel is compiled as PIE for bootloader 0.11's dynamic relocation.

## Testing

Unit tests run on the host target via `lib.rs` with `cfg(not(test))` gating:
```bash
cargo test -p openos-kernel --target x86_64-unknown-linux-gnu  # 649 tests
```

User-space crates (`shell_rs`, `test_sdk`, etc.) are excluded from workspace tests — they define `#![no_main]` + `#[panic_handler]` which conflict with `std`.

5 DMA tests are `#[ignore]` — they require physical memory mapping (bare-metal only).

## Syscall Summary (39 total)

| Range | Subsystem | Count |
|-------|-----------|-------|
| 0x01-0x05 | Channel (IPC) | 5 |
| 0x10-0x12 | Handle | 3 |
| 0x30-0x36 | Process + memory | 7 |
| 0x40-0x42 | Thread | 3 |
| 0xA0-0xA8 | Socket + DNS | 9 |
| 0xB0-0xB4 | Hardware access | 5 |
| 0xF0-0xFF | Console, event, fs, etc. | 7 |
