# OpenOS

**English** | [中文](#中文)

A microkernel operating system written in Rust, targeting x86_64 bare metal.

## Overview

OpenOS is a research microkernel OS that runs directly on hardware (or QEMU) with no underlying operating system. Written entirely in Rust with `#![no_std]` and `#![no_main]`, it leverages Rust's type system and ownership model to enforce memory safety at the kernel level — the most critical layer of any operating system.

**Key properties:**
- **Microkernel architecture** — minimal kernel (memory, scheduling, IPC); drivers and services run in user space
- **Memory safety** — Rust's borrow checker eliminates use-after-free, double-free, and buffer overflow bugs at compile time
- **Higher-half kernel** — mapped at `0xFFFFFFFF80100000`, leaving lower address space for user programs
- **Interrupt-driven** — hardware interrupts (timer, keyboard) via IDT + PIC 8259

## Quick Start

### Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust nightly | 1.99+ | `#![feature(abi_x86_interrupt)]`, `build-std` |
| QEMU | 8.x | x86_64 system emulation |
| NASM | 2.16+ | Assembler (bootloader) |
| LLD | 18+ | Linker (via `rust-lld`) |
| GDB | 15+ | Kernel debugging (optional) |

### Install Dependencies (Ubuntu/Debian)

```bash
# Rust nightly via TUNA mirror (China)
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
rustup install nightly
rustup component add rust-src llvm-tools-preview clippy rustfmt --toolchain nightly

# System packages
sudo apt install nasm lld llvm qemu-system-x86 gdb-multiarch xorriso mtools
```

### Build & Run

```bash
make build        # Compile kernel
make run          # Launch in QEMU (GTK display)
make run-serial   # Launch in QEMU (serial output, headless)
make debug        # QEMU + GDB attached
make check        # Full CI: fmt + clippy + build
make help         # All commands
```

### Raw Cargo Commands

Normal `cargo build` won't work — bare-metal requires nightly features:

```bash
cargo build -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem
cargo clippy -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem -- -D warnings
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
                   │   .bss       │   Zero-initialized data
                   ├──────────────┤
                   │   heap       │   Kernel allocator (100 KiB)
                   └──────────────┘
0x0000000000000000 ┌──────────────┐ ← User space (future)
                   │   ...        │
                   └──────────────┘
```

### Boot Sequence

```
BIOS → bootloader (0.9) → _start()
  │
  ├─ 1. VGA init        (drivers/vga.rs)      — clear screen, enable println!
  ├─ 2. GDT + TSS       (arch/x86_64/gdt.rs)  — segment descriptors, double-fault stack
  ├─ 3. IDT             (arch/x86_64/interrupts.rs) — exception + IRQ handlers
  ├─ 4. PIC init        (pic8259)             — remap IRQ 0-15 to INT 32-47
  ├─ 5. Enable interrupts                      — sti instruction
  ├─ 6. Heap allocator  (memory/allocator.rs)  — linked_list_allocator at 0x4444_4444_0000
  ├─ 7. Syscall handler (syscall/mod.rs)       — dispatcher (placeholder)
  ├─ 8. IPC subsystem   (ipc/mod.rs)           — port registry, message passing
  ├─ 9. Task scheduler  (task/scheduler.rs)    — round-robin, idle task
  └─ 10. Idle loop      → hlt instruction
```

### Module Map

```
src/
├── main.rs                  Kernel entry, panic handler, alloc error handler
├── arch/
│   └── x86_64/
│       ├── mod.rs           Architecture init orchestrator
│       ├── gdt.rs           GDT + TSS (double-fault IST stack, 20 KiB)
│       ├── interrupts.rs    IDT, PIC 8259, breakpoint/double-fault/timer/keyboard ISRs
│       └── linker.ld        Higher-half linker script (KERNEL_OFFSET = 0xFFFFFFFF80000000)
├── drivers/
│   ├── vga.rs               VGA text buffer (0xB8000), 80×25, green-on-black
│   └── serial.rs            UART 16550 (0x3F8), debug output to QEMU serial
├── memory/
│   └── allocator.rs         Heap allocator (linked_list_allocator), frame allocator placeholder
├── task/
│   ├── task.rs              TaskId (atomic), TaskState, Task control block
│   └── scheduler.rs         Round-robin scheduler, ready queue
├── syscall/
│   └── mod.rs               SyscallNumber enum, handle_syscall dispatcher
├── ipc/
│   └── mod.rs               Message/MessageData types, Port, IpcManager (BTreeMap)
└── fs/
    └── mod.rs               Placeholder for VFS
```

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust (nightly) | Memory safety without GC, zero-cost abstractions, `#![no_std]` support |
| Architecture | Microkernel | Minimal TCB, fault isolation, user-space drivers |
| Kernel model | Higher-half | Separates kernel/user address space, enables future user-space at 0x0 |
| Target | `x86_64-unknown-none` | Built-in bare-metal target, no custom JSON needed |
| Allocator | `linked_list_allocator` | Simple, no external dependencies, suitable for early kernel heap |
| Scheduling | Round-robin | Simple, fair, adequate for initial implementation |
| IPC | Message passing (ports) | Classic microkernel model (L4-inspired), extensible |
| Synchronization | `spin::Mutex` | No-std spinlock, widely used in Rust OS projects |
| Bootloader | `bootloader 0.9` | Mature, BIOS-based, handles page table setup |

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `bootloader` | 0.9 | BIOS bootloader, loads kernel, sets up paging |
| `x86_64` | 0.15 | CPU structures: GDT, IDT, paging, port I/O |
| `pic8259` | 0.11 | Intel 8259 PIC initialization |
| `uart_16550` | 0.3 | Serial port driver (COM1) |
| `spin` | 0.9 | Spinlock (`Mutex`) for `no_std` |
| `linked_list_allocator` | 0.10 | Kernel heap allocator |
| `lazy_static` | 1.0 | Lazy initialization of statics (GDT, IDT, etc.) |
| `volatile` | 0.2 | Volatile memory access (VGA buffer) |
| `pc-keyboard` | 0.7 | PS/2 keyboard scancode decoding |

## Development

### Adding a Device Driver

1. Create `src/drivers/<name>.rs`
2. Define I/O port constants, driver state struct, `init()` function
3. Add `pub mod <name>;` to `src/drivers/mod.rs`
4. Call `<name>::init()` from boot sequence
5. If IRQ-based: register handler in `src/arch/x86_64/interrupts.rs`

### Adding a System Call

1. Add variant to `SyscallNumber` in `src/syscall/mod.rs`
2. Add handler case in `handle_syscall()`
3. Document ABI: which register holds which argument

### Adding an IPC Service

1. Define request/response message types
2. Create port via `ipc::create_port()`
3. Implement `handle_message()` dispatcher
4. Register service in `ipc::init()`

## Lint & Code Quality

```bash
make lint         # clippy with -D warnings
make fmt          # cargo fmt --check
make check        # All of the above + build
```

Clippy configuration (`src/main.rs`):
```rust
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(dead_code, unused_variables)]  // scaffolding code
```

Formatting: `rustfmt.toml` with `group_imports = "StdExternalCrate"`.

## License

MIT OR Apache-2.0

---

# 中文

使用 Rust 编写的微内核操作系统，目标平台 x86_64 裸机。

## 概述

OpenOS 是一个研究型微内核操作系统，直接运行在硬件（或 QEMU）上，无需底层操作系统。全部代码使用 Rust 编写，采用 `#![no_std]` 和 `#![no_main]`，利用 Rust 的类型系统和所有权模型在内核层——操作系统最关键的层级——强制保证内存安全。

**核心特性：**
- **微内核架构** — 内核仅包含核心服务（内存管理、调度、IPC）；驱动和服务运行在用户态
- **内存安全** — Rust 的借用检查器在编译期消除释放后使用、重复释放和缓冲区溢出漏洞
- **高半内核** — 映射在 `0xFFFFFFFF80100000`，低地址空间留给用户程序
- **中断驱动** — 通过 IDT + PIC 8259 处理硬件中断（定时器、键盘）

## 快速开始

### 环境要求

| 工具 | 版本 | 用途 |
|------|------|------|
| Rust nightly | 1.99+ | `#![feature(abi_x86_interrupt)]`、`build-std` |
| QEMU | 8.x | x86_64 系统模拟 |
| NASM | 2.16+ | 汇编器（引导程序） |
| LLD | 18+ | 链接器（通过 `rust-lld`） |
| GDB | 15+ | 内核调试（可选） |

### 安装依赖 (Ubuntu/Debian)

```bash
# Rust nightly（使用 TUNA 镜像加速）
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
rustup install nightly
rustup component add rust-src llvm-tools-preview clippy rustfmt --toolchain nightly

# 系统包
sudo apt install nasm lld llvm qemu-system-x86 gdb-multiarch xorriso mtools
```

### 构建与运行

```bash
make build        # 编译内核
make run          # QEMU 启动（GTK 显示）
make run-serial   # QEMU 启动（串口输出，无头模式）
make debug        # QEMU + GDB 调试
make check        # 完整 CI：fmt + clippy + build
make help         # 查看所有命令
```

### 原始 Cargo 命令

普通 `cargo build` 无法工作——裸机需要 nightly 特性：

```bash
cargo build -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem
cargo clippy -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem -- -D warnings
```

## 架构

### 内核布局（高半地址）

```
虚拟地址空间：
0xFFFFFFFF80100000 ┌──────────────┐ ← 内核 .text
                   │   .text      │   代码段（4K 对齐）
                   ├──────────────┤
                   │   .rodata    │   只读数据
                   ├──────────────┤
                   │   .data      │   已初始化数据
                   ├──────────────┤
                   │   .bss       │   零初始化数据
                   ├──────────────┤
                   │   heap       │   内核堆分配器（100 KiB）
                   └──────────────┘
0x0000000000000000 ┌──────────────┐ ← 用户空间（未来）
                   │   ...        │
                   └──────────────┘
```

### 启动流程

```
BIOS → bootloader (0.9) → _start()
  │
  ├─ 1. VGA 初始化     (drivers/vga.rs)      — 清屏，启用 println!
  ├─ 2. GDT + TSS      (arch/x86_64/gdt.rs)  — 段描述符，双重故障栈
  ├─ 3. IDT            (arch/x86_64/interrupts.rs) — 异常 + IRQ 处理
  ├─ 4. PIC 初始化     (pic8259)             — 重映射 IRQ 0-15 → INT 32-47
  ├─ 5. 开启中断       — sti 指令
  ├─ 6. 堆分配器       (memory/allocator.rs)  — linked_list_allocator @ 0x4444_4444_0000
  ├─ 7. 系统调用处理   (syscall/mod.rs)       — 分发器（占位）
  ├─ 8. IPC 子系统     (ipc/mod.rs)           — 端口注册，消息传递
  ├─ 9. 任务调度器     (task/scheduler.rs)    — 轮询调度，空闲任务
  └─ 10. 空闲循环      → hlt 指令
```

### 模块结构

```
src/
├── main.rs                  内核入口、panic 处理、alloc 错误处理
├── arch/
│   └── x86_64/
│       ├── mod.rs           架构初始化编排器
│       ├── gdt.rs           GDT + TSS（双重故障 IST 栈，20 KiB）
│       ├── interrupts.rs    IDT、PIC 8259、断点/双重故障/定时器/键盘 ISR
│       └── linker.ld        高半内核链接脚本（KERNEL_OFFSET = 0xFFFFFFFF80000000）
├── drivers/
│   ├── vga.rs               VGA 文本缓冲区（0xB8000），80×25，绿字黑底
│   └── serial.rs            UART 16550（0x3F8），调试输出到 QEMU 串口
├── memory/
│   └── allocator.rs         堆分配器（linked_list_allocator），帧分配器占位
├── task/
│   ├── task.rs              TaskId（原子）、TaskState、任务控制块
│   └── scheduler.rs         轮询调度器，就绪队列
├── syscall/
│   └── mod.rs               SyscallNumber 枚举，handle_syscall 分发器
├── ipc/
│   └── mod.rs               Message/MessageData 类型，Port，IpcManager（BTreeMap）
└── fs/
    └── mod.rs               VFS 占位符
```

### 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 语言 | Rust (nightly) | 无 GC 的内存安全、零成本抽象、`#![no_std]` 支持 |
| 架构 | 微内核 | 最小 TCB、故障隔离、用户态驱动 |
| 内核模型 | 高半内核 | 分离内核/用户地址空间，0x0 留给用户态 |
| 目标 | `x86_64-unknown-none` | 内置裸机目标，无需自定义 JSON |
| 分配器 | `linked_list_allocator` | 简单、无外部依赖、适合早期内核堆 |
| 调度 | 轮询调度 | 简单、公平、满足初始实现需求 |
| IPC | 消息传递（端口） | 经典微内核模型（L4 启发），可扩展 |
| 同步 | `spin::Mutex` | 无标准库自旋锁，Rust OS 项目广泛使用 |
| 引导 | `bootloader 0.9` | 成熟、基于 BIOS、处理页表设置 |

## 开发指南

### 添加设备驱动

1. 创建 `src/drivers/<name>.rs`
2. 定义 I/O 端口常量、驱动状态结构体、`init()` 函数
3. 在 `src/drivers/mod.rs` 添加 `pub mod <name>;`
4. 在启动流程中调用 `<name>::init()`
5. 如需中断：在 `src/arch/x86_64/interrupts.rs` 注册处理函数

### 添加系统调用

1. 在 `src/syscall/mod.rs` 的 `SyscallNumber` 添加变体
2. 在 `handle_syscall()` 添加处理分支
3. 记录 ABI：哪个寄存器存放哪个参数

### 添加 IPC 服务

1. 定义请求/响应消息类型
2. 通过 `ipc::create_port()` 创建端口
3. 实现 `handle_message()` 分发器
4. 在 `ipc::init()` 注册服务

## 代码质量

```bash
make lint         # clippy -D warnings
make fmt          # cargo fmt --check
make check        # 以上全部 + 编译
```

Clippy 配置（`src/main.rs`）：
```rust
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(dead_code, unused_variables)]  // 脚手架代码
```

格式化：`rustfmt.toml`，`group_imports = "StdExternalCrate"`。

## 许可证

MIT OR Apache-2.0
