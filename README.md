# OpenOS

<p align="center">
  <img src="assets/mascot.webp" alt="OpenOS Mascot" width="200">
</p>

**English** | [中文](#中文)

A microkernel operating system written in Rust, targeting x86_64 bare metal.

## Overview

OpenOS is a research microkernel OS that runs directly on hardware (or QEMU) with no underlying operating system. Written entirely in Rust with `#![no_std]` and `#![no_main]`, it leverages Rust's type system and ownership model to enforce memory safety at the kernel level.

**Key properties:**
- **Microkernel architecture** — minimal kernel (memory, scheduling, IPC, VFS, networking); drivers can run in user space
- **Memory safety** — Rust's borrow checker eliminates use-after-free, double-free, and buffer overflow bugs at compile time
- **Capability-based security** — Handle system with 28-bit slot / 10-bit rights / 26-bit generation
- **SMP support** — per-CPU run queues, IPI-based wakeup, work stealing, ACPI/MADT parsing
- **Full TCP/IP stack** — Ethernet, ARP, IPv4, TCP, UDP, DHCP with lease renewal, DNS, BSD socket API
- **ext2 filesystem** — read/write support with block cache, unlink/rename/stat/readdir syscalls
- **BIOS + UEFI boot** — dual-mode disk image builder
- **Dynamic linking** — ELF PT_DYNAMIC parser, user-space ld.so
- **59 coreutils** — Linux-like command set (ls, cat, grep, sort, wc, hexdump, etc.)
- **45 syscalls** — IPC, process, memory, filesystem, network, device, thread

## Quick Start

### Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust nightly | 1.99+ | `build-std` for bare-metal target |
| QEMU | 8.x | x86_64 system emulation |
| NASM | 2.16+ | Assembler for user-space programs |

### Install Dependencies (Ubuntu/Debian)

```bash
# Rust nightly
rustup install nightly
rustup component add rust-src llvm-tools-preview clippy rustfmt --toolchain nightly

# System packages
sudo apt install nasm lld llvm qemu-system-x86 gdb-multiarch mtools
```

### Build & Run

```bash
make build        # Build kernel + BIOS disk image
make run          # Launch in QEMU (serial output)
make run-gui      # Launch in QEMU (graphical display)
make build-uefi   # Build UEFI disk image
make run-uefi     # Launch in QEMU with OVMF (UEFI)
make debug        # QEMU + GDB attached
make check        # Full CI: fmt + clippy + build
make test         # Unit tests + quality checks
make help         # All commands
```

### Raw Cargo Commands

Normal `cargo build` won't work — bare-metal requires nightly features:

```bash
# Build kernel
cargo build -p openos-kernel --target x86_64-unknown-none \
  -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem

# Run tests (host target)
cargo test -p openos-kernel --target x86_64-unknown-linux-gnu
```

## Architecture

### Kernel Layout (Higher-Half)

```
Virtual Address Space:
0xFFFFFFFF80100000 ┌──────────────┐ ← Kernel .text
                   │   .text      │   Code (4K aligned)
                   ├──────────────┤
                   │   .rodata    │   Read-only data
                   ├──────────────┤
                   │   .data      │   Initialized data
                   ├──────────────┤
                   │   .bss       │   Zero-initialized data (2 MiB heap)
                   └──────────────┘
0x0000_8000_0000_0000 ┌──────────┐ ← User space ceiling (128 TiB)
                      │  stack   │   User stack (8 KiB, grows down)
                      │  mmap    │   Memory-mapped regions
                      │  heap    │   Program break (sys_brk)
                      │  .bss    │   Zero-fill
                      │  .data   │   Initialized data
                      │  .text   │   Code + PLT/GOT
0x0000_0000_0000_0000 └──────────┘
```

### Boot Sequence

```
BIOS/UEFI → bootloader (0.11) → kernel_main(boot_info)
  │
  ├─ 1. Serial init         — UART 16550 at COM1
  ├─ 2. phys_offset store   — physical_memory_offset from BootInfo
  ├─ 3. VGA/Framebuffer     — pixel font text renderer
  ├─ 4. GDT + IDT + PIC    — segment descriptors, ISR handlers
  ├─ 5. SYSCALL MSRs        — STAR/LSTAR/SFMASK/EFER configuration
  ├─ 6. Heap allocator      — 2 MiB linked_list_allocator
  ├─ 7. Frame allocator     — bitmap allocator (32-64 MiB region)
  ├─ 8. VFS + ramfs         — mount ramfs at "/"
  ├─ 9. VirtIO-Block        — PCI discovery, mount ext2 at "/disk"
  ├─ 10. Network            — VirtIO-Net driver, DHCP negotiation
  ├─ 11. IPC subsystem      — channel message passing
  ├─ 12. Per-CPU data       — GSBASE MSR for CPU 0
  ├─ 13. Scheduler          — SMP round-robin with per-CPU queues
  ├─ 14. IRQ forwarding     — keyboard IRQ 1 → IrqEvent
  └─ 15. Load first process — ELF from initrd, IRETQ to Ring 3
```

### Module Map

```
kernel/src/
├── main.rs                  Kernel entry, boot sequence orchestration
├── lib.rs                   Module root (cfg-gated for test support)
├── elf.rs                   ELF64 parser + loader (PT_LOAD, PT_DYNAMIC, relocations)
├── initrd.rs                Initrd archive parser (magic "OSRD")
├── frame_alloc.rs           Bitmap frame allocator (4 KiB physical frames)
├── handle.rs                Capability-based Handle system
├── sync.rs                  Interrupt-safe mutex (IntMutex)
├── arch/x86_64/
│   ├── gdt.rs               GDT + TSS (user segments, double-fault IST)
│   ├── interrupts.rs        IDT, PIC 8259, exception/IRQ handlers
│   ├── syscall.rs           SYSCALL/SYSRET entry stub with CR3 switching
│   ├── acpi.rs              RSDP/RSDT/XSDT/MADT parser (BIOS + UEFI)
│   ├── apic.rs              Local APIC + I/O APIC drivers
│   ├── ap_start.rs          AP boot via INIT+SIPI IPI
│   └── percpu.rs            Per-CPU data via GSBASE MSR
├── drivers/
│   ├── serial.rs            UART 16550 (COM1, 0x3F8)
│   ├── vga.rs               Framebuffer text renderer (8×16 bitmap font)
│   ├── keyboard.rs          PS/2 keyboard driver (scancode decoding)
│   ├── pci.rs               PCI bus scanner
│   ├── virtio_block.rs      VirtIO-Block device driver
│   ├── virtio_net.rs        VirtIO-Net device driver
│   ├── block.rs             Block device abstraction + registry
│   ├── net.rs               Network driver interface
│   └── font_8x16.rs         Bitmap font data
├── memory/
│   ├── mod.rs               phys_to_virt, page table creation/switching
│   ├── allocator.rs         Kernel heap allocator (2 MiB)
│   ├── pagetable.rs         Unified page table abstraction (map/unmap/translate)
│   ├── vma.rs               Virtual Memory Area tracker
│   └── dma.rs               DMA buffer allocation (physical contiguity)
├── task/
│   ├── task.rs              Task control block, SavedContext (136 bytes)
│   ├── scheduler.rs         SMP round-robin scheduler (8 CPUs, work stealing)
│   └── user.rs              ELF loading, page table setup, Ring 3 transition
├── syscall/
│   ├── mod.rs               Syscall dispatcher (39 syscalls)
│   └── number.rs            Syscall number constants
├── ipc/mod.rs               Channel message passing with handle transfer
├── fs/
│   ├── vfs.rs               VFS trait + mount point dispatch
│   ├── ramfs.rs             In-memory ramfs
│   ├── ext2.rs              ext2 filesystem (read/write)
│   └── block_cache.rs       LRU block cache (64 entries)
└── net/
    ├── mod.rs               Ethernet/ARP/IPv4 dispatch
    ├── tcp.rs               TCP state machine (RFC 793, 10 states)
    ├── udp.rs               UDP protocol
    ├── dhcp.rs              DHCP client
    ├── dns.rs               DNS resolver (RFC 1035)
    └── socket.rs            BSD socket abstraction
```

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust (nightly) | Memory safety without GC, zero-cost abstractions, `#![no_std]` support |
| Architecture | Microkernel | Minimal TCB, fault isolation, user-space drivers |
| Kernel model | Higher-half | Separates kernel/user address space, enables user-space at 0x0 |
| Target | `x86_64-unknown-none` | Built-in bare-metal target, no custom JSON needed |
| Bootloader | `bootloader 0.11` | BIOS + UEFI support, dynamic physical memory mapping |
| Allocator | `linked_list_allocator` | Simple, no external dependencies, suitable for kernel heap |
| Frame allocator | Bitmap | Fixed-size bitmap, O(n) first-fit, 8192 frames (32 MiB) |
| Scheduling | SMP round-robin | Per-CPU queues, IPI wakeup, work stealing for load balancing |
| IPC | Channel message passing | Synchronous, bidirectional, handle transfer (L4-inspired) |
| Security | Capability-based | Handle with rights bitmask, monotonic privilege reduction |
| Synchronization | `spin::Mutex` + `IntMutex` | No-std spinlock; IntMutex disables interrupts during lock |
| Page table | 4-level x86_64 | Per-process P4 with kernel entries shared, CR3 on context switch |

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `bootloader_api` | 0.11 | Kernel boot interface (BootInfo, entry_point!) |
| `bootloader` | 0.11 | Disk image builder (BiosBoot + UefiBoot) |
| `x86_64` | 0.15 | CPU structures: GDT, IDT, paging, port I/O |
| `pic8259` | 0.11 | Intel 8259 PIC initialization |
| `uart_16550` | 0.3 | Serial port driver (COM1) |
| `spin` | 0.9 | Spinlock (`Mutex`) for `no_std` |
| `linked_list_allocator` | 0.10 | Kernel heap allocator |
| `pc-keyboard` | 0.7 | PS/2 keyboard scancode decoding |

## Development

### Adding a Device Driver

1. Create `kernel/src/drivers/<name>.rs`
2. Define I/O port constants, driver state struct, `init()` function
3. Add `pub mod <name>;` to `kernel/src/drivers/mod.rs`
4. Call `<name>::init()` from boot sequence in `kernel/src/main.rs`
5. If IRQ-based: register handler in `kernel/src/arch/x86_64/interrupts.rs`

### Adding a System Call

1. Add constant to `kernel/src/syscall/number.rs`
2. Add import and dispatch case in `kernel/src/syscall/mod.rs`
3. Implement handler function
4. Add SDK wrapper in `sdk/src/lib.rs`

### Adding a User-Space Program

1. Create `user/<name>/src/main.rs` with `#![no_std]` + `#![no_main]`
2. Add to workspace members in root `Cargo.toml`
3. Add build/copy steps to Makefile `user-rs` target
4. Add to initrd in Makefile `initrd` target

## Testing

```bash
make test              # Unit tests + quality checks
cargo test -p openos-kernel --target x86_64-unknown-linux-gnu  # 649 unit tests
bash tests/integration.sh   # 30 QEMU integration tests
bash tests/scenarios.sh     # 48 scenario tests
bash tests/edge_cases.sh    # 43 edge case tests
bash tests/quality.sh       # 10 quality checks
```

## Lint & Code Quality

```bash
make lint         # clippy with -D warnings
make fmt          # cargo fmt --check
make check        # All of the above + build
```

Clippy configuration (`kernel/src/lib.rs`):
```rust
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
```

Formatting: `rustfmt.toml` with `group_imports = "StdExternalCrate"`.

## License

MIT OR Apache-2.0

---

# 中文

使用 Rust 编写的微内核操作系统，目标平台 x86_64 裸机。

## 概述

OpenOS 是一个研究型微内核操作系统，直接运行在硬件（或 QEMU）上，无需底层操作系统。全部代码使用 Rust 编写，采用 `#![no_std]` 和 `#![no_main]`。

**核心特性：**
- **微内核架构** — 内核仅包含核心服务；驱动和服务可运行在用户态
- **内存安全** — Rust 借用检查器在编译期消除内存安全漏洞
- **能力安全** — Handle 系统（28位槽/10位权限/26位代际）
- **SMP 支持** — per-CPU 运行队列、IPI 唤醒、工作窃取、ACPI/MADT 解析
- **完整 TCP/IP 栈** — 以太网、ARP、IPv4、TCP、UDP、DHCP、DNS、BSD Socket API
- **ext2 文件系统** — 读写支持，带块缓存
- **BIOS + UEFI 引导** — 双模式磁盘镜像构建器
- **动态链接** — ELF PT_DYNAMIC 解析器、用户态 ld.so

## 快速开始

### 环境要求

| 工具 | 版本 | 用途 |
|------|------|------|
| Rust nightly | 1.99+ | `build-std` 裸机构建 |
| QEMU | 8.x | x86_64 系统模拟 |
| NASM | 2.16+ | 用户态程序汇编器 |

### 构建与运行

```bash
make build        # 构建内核 + BIOS 磁盘镜像
make run          # QEMU 启动（串口输出）
make run-gui      # QEMU 启动（图形显示）
make build-uefi   # 构建 UEFI 磁盘镜像
make run-uefi     # QEMU + OVMF 启动（UEFI）
make check        # 完整 CI：fmt + clippy + build
make test         # 单元测试 + 质量检查
make help         # 查看所有命令
```

## 许可证

MIT OR Apache-2.0
