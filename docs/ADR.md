# Architecture Decision Records / 架构决策记录

This document records the significant architectural decisions made during the development of OpenOS.
本文档记录 OpenOS 开发过程中的重大架构决策。

Each ADR follows this format:
每条 ADR 遵循以下格式：

- **Status**: Proposed | Accepted | Deprecated | Superseded
- **Context**: What is the issue that motivates this decision?
- **Decision**: What is the change being proposed?
- **Consequences**: What are the resulting implications?

---

## ADR-001: Language Choice — Rust (Nightly)

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

An OS kernel must manage hardware directly — memory, CPU, I/O. Traditional choices are C and assembly. C lacks memory safety; assembly is error-prone and non-portable. We need a language that provides:
内核必须直接管理硬件——内存、CPU、I/O。传统选择是 C 和汇编。C 缺乏内存安全；汇编易错且不可移植。我们需要一种语言提供：

1. Memory safety without runtime overhead
2. Zero-cost abstractions for hardware interaction
3. `#![no_std]` support (no standard library dependency)
4. Strong type system for state machine modeling

### Decision / 决策

Use Rust nightly toolchain with `#![no_std]` and `#![no_main]`.
使用 Rust nightly 工具链，配合 `#![no_std]` 和 `#![no_main]`。

Key nightly features used:
使用的 nightly 特性：

- `abi_x86_interrupt` — allows `extern "x86-interrupt"` for ISR functions
- `alloc_error_handler` — custom allocation failure handler
- `build-std` — rebuild `core` and `alloc` for bare-metal target

### Consequences / 影响

**Positive:**
- Borrow checker catches use-after-free, data races at compile time
- `#[repr(C)]` structs map directly to hardware registers
- Pattern matching ensures exhaustive handling of CPU exceptions
- `unsafe` boundary clearly marks hardware interaction points

**Negative:**
- Nightly-only features may break across toolchain updates
- `unsafe` is still required for raw hardware access
- Ecosystem maturity for OS dev is lower than C

---

## ADR-002: Kernel Architecture — Microkernel

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

Monolithic kernels (Linux, Windows NT) run all services in kernel space — a bug in a driver can crash the entire system. Microkernels (L4, seL4, MINIX) run only essential services in kernel space, isolating faults.
宏内核（Linux、Windows NT）将所有服务运行在内核态——驱动中的 bug 可以导致整个系统崩溃。微内核（L4、seL4、MINIX）仅在内核态运行核心服务，隔离故障。

### Decision / 决策

Adopt microkernel architecture. The kernel handles only:
采用微内核架构。内核仅处理：

| Service | Kernel-space | User-space (future) |
|---------|:---:|:---:|
| Memory management | ✓ | |
| Task scheduling | ✓ | |
| IPC | ✓ | |
| Interrupt dispatch | ✓ | |
| Device drivers | | ✓ |
| Filesystem | | ✓ |
| Network stack | | ✓ |
| Display server | | ✓ |

### Consequences / 影响

**Positive:**
- Smaller trusted computing base (TCB)
- Driver bugs don't crash the kernel
- Services can be restarted independently
- Easier to formally verify (see seL4)

**Negative:**
- IPC overhead for every system call
- More complex boot sequence (user-space services must be loaded)
- Harder to achieve performance parity with monolithic kernels

---

## ADR-003: Memory Model — Higher-Half Kernel

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

The x86_64 virtual address space is 48 bits (256 TiB). The kernel can be placed at the bottom (0x0) or top (0xFFFF...) of the address space. Placing it at the top (higher-half) is the standard approach for modern OSes.
x86_64 虚拟地址空间为 48 位（256 TiB）。内核可以放置在地址空间底部（0x0）或顶部（0xFFFF...）。放置在顶部（高半）是现代操作系统的标准做法。

### Decision / 决策

Higher-half kernel mapped at `0xFFFFFFFF80100000`.
高半内核映射在 `0xFFFFFFFF80100000`。

```
KERNEL_OFFSET = 0xFFFFFFFF80000000
Physical load address: 0x100000 (1 MiB, conventional)
Virtual base: KERNEL_OFFSET + 0x100000 = 0xFFFFFFFF80100000
```

Linker script (`src/arch/x86_64/linker.ld`) uses `AT(ADDR(.section) - KERNEL_OFFSET)` to emit physical addresses in the binary while the kernel runs at virtual addresses.
链接脚本使用 `AT(ADDR(.section) - KERNEL_OFFSET)` 在二进制中输出物理地址，而内核在虚拟地址运行。

### Consequences / 影响

**Positive:**
- Lower 2 GiB is free for user-space programs (can use `call`, `jmp` with 32-bit offsets)
- Kernel is protected by page tables — user-space page fault doesn't touch kernel pages
- Matches Linux, Windows, and most modern OS layouts

**Negative:**
- Requires working page tables before the kernel can run at its virtual address
- Bootloader must map the kernel's physical pages to the higher-half region

---

## ADR-004: Build Target — `x86_64-unknown-none`

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

Bare-metal Rust requires a target specification that disables OS-specific features (no libc, no threads, no filesystem). Options: custom JSON target vs built-in.
裸机 Rust 需要一个禁用操作系统特性的目标规范（无 libc、无线程、无文件系统）。选项：自定义 JSON 目标 vs 内置目标。

### Decision / 决策

Use the built-in `x86_64-unknown-none` target instead of a custom JSON file.
使用内置 `x86_64-unknown-none` 目标，而非自定义 JSON 文件。

Configuration in `.cargo/config.toml`:
```toml
[build]
target = "x86_64-unknown-none"

[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
```

### Consequences / 影响

**Positive:**
- No custom target JSON to maintain
- Works with `cargo build` after setting `.cargo/config.toml`
- Rust team maintains the target spec

**Negative:**
- Cannot customize features (e.g., cannot disable SSE without custom JSON)
- The `-mmx,-sse,-sse2` features in the unused `x86_64-openos.json` required a custom spec — but the built-in target handles this correctly

---

## ADR-005: Synchronization — Spinlocks

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

The kernel needs mutual exclusion for shared data structures (scheduler queue, IPC port registry, VGA buffer). Options: spinlock, ticket lock, MCS lock, or interrupt-disabling lock.
内核需要对共享数据结构（调度器队列、IPC 端口注册表、VGA 缓冲区）进行互斥访问。选项：自旋锁、票据锁、MCS 锁或禁中断锁。

### Decision / 决策

Use `spin::Mutex` from the `spin` crate (v0.9).
使用 `spin` crate (v0.9) 的 `spin::Mutex`。

For VGA output, combine with `x86_64::instructions::interrupts::without_interrupts` to prevent deadlocks from interrupt handlers trying to print.
对于 VGA 输出，配合 `without_interrupts` 使用，防止中断处理函数尝试打印时死锁。

### Consequences / 影响

**Positive:**
- Simple, well-understood
- No dependency on OS features (works in `no_std`)
- Deterministic — no sleeping, no scheduler interaction

**Negative:**
- Busy-waits — wastes CPU cycles
- Not fair — no ordering guarantee for waiters
- Can deadlock if a spinlock is held when an interrupt fires on the same CPU (mitigated by `without_interrupts` for print locks)

---

## ADR-006: IPC Model — Message Passing with Ports

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

Microkernels require efficient inter-process communication. Two main models: shared memory and message passing. L4 (the gold standard for microkernel IPC) uses synchronous message passing.
微内核需要高效的进程间通信。两种主要模型：共享内存和消息传递。L4（微内核 IPC 的黄金标准）使用同步消息传递。

### Decision / 决策

Implement port-based message passing:
实现基于端口的消息传递：

- Each service owns a **port** (identified by `u64` ID)
- Messages are `Message { sender, receiver, data: MessageData }`
- `MessageData` enum supports: `Text`, `Bytes`, `Request { id, data }`, `Response { id, data }`
- Ports stored in `BTreeMap<u64, Port>` inside `IpcManager`
- Send is asynchronous (message queued in receiver's inbox)
- Receive is synchronous (blocks until message available — not yet implemented)

### Consequences / 影响

**Positive:**
- Clean separation between sender and receiver
- Request/Response pattern built into the message type
- Extensible — can add capability-based access control later

**Negative:**
- Copy-based (no zero-copy yet) — large messages are expensive
- No shared memory for bulk data transfer (e.g., framebuffer)
- BTreeMap lookup is O(log n) — could use array for known port count

---

## ADR-007: Panic Strategy — Abort

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

Rust panics can either unwind the stack (default) or abort immediately. Unwinding requires significant runtime support (landing pads, exception tables) that's complex to implement in a bare-metal kernel.
Rust panic 可以选择栈展开（默认）或立即中止。展开需要大量运行时支持（着陆垫、异常表），在裸机内核中实现复杂。

### Decision / 决策

Set `panic = "abort"` in both `[profile.dev]` and `[profile.release]`.
在 dev 和 release profile 中均设置 `panic = "abort"`。

```toml
[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
opt-level = "s"
lto = true
```

### Consequences / 影响

**Positive:**
- No unwinding tables — smaller binary
- No need to implement `eh_personality` or landing pads
- Simpler panic handler — just print and halt

**Negative:**
- Cannot catch panics with `catch_unwind`
- Any panic is fatal — no graceful degradation
- Libraries that rely on unwinding won't work correctly

---

## ADR-008: Heap Allocator — `linked_list_allocator`

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

The kernel needs dynamic memory allocation (`Box`, `Vec`, `String`). Options: bump allocator, linked-list allocator, slab allocator, buddy allocator.
内核需要动态内存分配（`Box`、`Vec`、`String`）。选项：bump 分配器、链表分配器、slab 分配器、伙伴分配器。

### Decision / 决策

Use `linked_list_allocator` crate (v0.10) as the global allocator.
使用 `linked_list_allocator` crate (v0.10) 作为全局分配器。

```rust
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init_heap() {
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }
}
```

Heap: 100 KiB starting at virtual address `0x4444_4444_0000`.

### Consequences / 影响

**Positive:**
- Simple, no external dependencies
- First-fit allocation — reasonable for kernel use
- Thread-safe (wrapped in `spin::Mutex`)

**Negative:**
- Fragmentation over time — not suitable for long-running production kernel
- No per-CPU caches — lock contention under heavy allocation
- Fixed heap size (100 KiB) — must be made dynamic with page allocator

---

## ADR-009: VGA Output — Direct Buffer Access

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

The kernel needs output for debugging and user feedback. VGA text mode provides an 80×25 character buffer at physical address `0xB8000`. Alternatives: serial port, framebuffer, VESA.
内核需要输出用于调试和用户反馈。VGA 文本模式在物理地址 `0xB8000` 提供 80×25 字符缓冲区。替代方案：串口、帧缓冲、VESA。

### Decision / 决策

Implement both VGA text buffer and serial port output:
同时实现 VGA 文本缓冲区和串口输出：

- **VGA** (`drivers/vga.rs`): Direct write to `0xB8000` via `Volatile<ScreenChar>`, provides `print!`/`println!` macros
- **Serial** (`drivers/serial.rs`): UART 16550 at `0x3F8`, provides `serial_print!`/`serial_println!` macros

VGA uses `volatile` crate to prevent compiler from optimizing away writes.
VGA 使用 `volatile` crate 防止编译器优化掉写操作。

### Consequences / 影响

**Positive:**
- Immediate visual feedback during boot
- Serial output visible in QEMU with `-serial stdio`
- `Volatile` wrapper prevents reordering/elimination

**Negative:**
- VGA is not available on modern hardware without legacy support
- Direct buffer access bypasses any abstraction — hard to replace later
- No scrolling buffer management beyond simple line-shift

---

## ADR-010: Lint Configuration — Strict Clippy

**Status:** Accepted
**Date:** 2026-07-23

### Context / 背景

Kernel code must be correct — bugs are catastrophic. Rust's clippy linter catches common mistakes. We need to balance strictness with the reality of scaffolding code (dead code, unused variables).
内核代码必须正确——bug 是灾难性的。Rust 的 clippy 检查器捕获常见错误。需要在严格性和脚手架代码（dead code、unused variables）之间取得平衡。

### Decision / 决策

Enable strict clippy in `src/main.rs`:
在 `src/main.rs` 中启用严格 clippy：

```rust
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::module_inception,
    dead_code,
    unused_imports,
    unused_variables,
    clippy::missing_const_for_fn,
    clippy::used_underscore_items
)]
```

Formatting enforced by `rustfmt.toml` with `group_imports = "StdExternalCrate"`.

CI gate: `make check` runs fmt → clippy (-D warnings) → build.

### Consequences / 影响

**Positive:**
- Catches bugs early (e.g., `uninlined_format_args`, `use_self`)
- Consistent code style across contributors
- `missing_docs` ensures all public APIs are documented

**Negative:**
- `dead_code` allowed at crate level — some warnings suppressed
- `clippy::nursery` has unstable lints that may change behavior
- Must run `cargo fmt` before committing (manual step until CI is set up)
