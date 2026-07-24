# OpenOS System Interface Reference

**English** | [中文](#中文)

> This document defines the kernel–user-space ABI, IPC protocol, memory layout,
> and driver interface for OpenOS. All interfaces are versioned; breaking changes
> increment the ABI version.

---

## Table of Contents / 目录

1. [System Call ABI](#1-system-call-abi)
2. [System Call Reference](#2-system-call-reference)
3. [IPC Protocol](#3-ipc-protocol)
4. [Memory Layout](#4-memory-layout)
5. [Task Interface](#5-task-interface)
6. [Driver Interface](#6-driver-interface)
7. [Error Handling](#7-error-handling)

---

## 1. System Call ABI

### 1.1 Calling Convention

System calls use the `syscall` instruction (fastest user→kernel transition on `x86_64`).

| Register | On `syscall` entry | On `sysretq` return |
|----------|-------------------|---------------------|
| `RAX` | Syscall number | Return value |
| `RDI` | Argument 1 | Preserved |
| `RSI` | Argument 2 | Preserved |
| `RDX` | Argument 3 | Preserved |
| `R10` | Argument 4 (reserved) | Preserved |
| `RCX` | *Overwritten by CPU* (user RIP) | *Overwritten by CPU* |
| `R11` | *Overwritten by CPU* (user RFLAGS) | *Overwritten by CPU* |

The CPU automatically:
- Saves `RIP → RCX`, `RFLAGS → R11`
- Loads `CS/SS` from the STAR MSR (kernel segments)
- Clears `IF` in `RFLAGS` (interrupts disabled)

### 1.2 Return Convention

```
RAX = result                  (on success)
RAX = error_code | (1 << 63)  (on error, high bit set)
```

The high bit of `RAX` acts as an error flag. User-space must check bit 63
before interpreting the value as a result.

### 1.3 User-Space Assembly Template

```asm
; OpenOS syscall wrapper
; Inputs: rax = number, rdi = arg1, rsi = arg2, rdx = arg3
; Output: rax = result (check bit 63 for error)
openos_syscall:
    mov r10, rcx        ; save arg4 if needed
    syscall
    ret
```

### 1.4 C/Assembly ABI Summary

| Parameter | Register | Size |
|-----------|----------|------|
| Number | `RAX` | u64 |
| Arg 1 | `RDI` | u64 |
| Arg 2 | `RSI` | u64 |
| Arg 3 | `RDX` | u64 |
| Arg 4 | `R10` | u64 (reserved) |
| Return | `RAX` | u64 |

---

## 2. System Call Reference

### 2.1 `SYS_WRITE` — Write to Console

Write bytes to the kernel's VGA console output.

| Field | Value |
|-------|-------|
| Number | `1` |
| Arg 1 | `buf` — pointer to byte buffer in user-space |
| Arg 2 | `len` — number of bytes to write |
| Returns | Number of bytes written, or error |

**Errors:**
| Code | Name | Condition |
|------|------|-----------|
| 1 | `EINVAL` | `buf == NULL` or `len == 0` |

**Example (x86_64 asm):**
```asm
section .data
    msg: db "Hello from user-space!", 10
    msg_len equ $ - msg

section .text
    mov rax, 1              ; SYS_WRITE
    lea rdi, [rel msg]      ; buffer pointer
    mov rsi, msg_len        ; length
    syscall
```

### 2.2 `SYS_READ` — Read from Input

Read bytes from the keyboard input buffer into user-space.

| Field | Value |
|-------|-------|
| Number | `2` |
| Arg 1 | `buf` — pointer to destination buffer in user-space |
| Arg 2 | `len` — maximum bytes to read |
| Returns | Number of bytes actually read, or error |

**Errors:**
| Code | Name | Condition |
|------|------|-----------|
| 1 | `EINVAL` | `buf == NULL` or `len == 0` |
| 2 | `EAGAIN` | No input available (non-blocking) |

**Notes:**
- Currently unimplemented (returns 0).
- Will block until input is available once keyboard driver is complete.

### 2.3 `SYS_EXIT` — Terminate Process

Terminate the calling process. The kernel reclaims all resources.

| Field | Value |
|-------|-------|
| Number | `3` |
| Arg 1 | `status` — exit code (0 = success) |
| Returns | Does not return |

**Notes:**
- The `sysretq` after this syscall is never executed.
- The kernel halts the CPU until a full task cleanup is implemented.

### 2.4 `SYS_YIELD` — Yield CPU

Voluntarily yield the CPU to the next task in the scheduler.

| Field | Value |
|-------|-------|
| Number | `4` |
| Returns | `0` (always succeeds) |

**Notes:**
- Used for cooperative scheduling.
- With preemptive scheduling, this is a hint — the timer interrupt
  will preempt regardless.

---

## 3. IPC Protocol

### 3.1 Architecture

```
┌─────────────┐     Message      ┌─────────────┐
│  Task A     │ ───────────────→ │  Task B     │
│  (sender)   │                  │  (receiver) │
└─────────────┘                  └─────────────┘
       │                                │
       ▼                                ▼
   Port ID (src)                   Port ID (dst)
                                   Inbox (FIFO)
```

Each task creates one or more **ports** (mailboxes). Messages are routed by
port ID. The kernel copies the message from sender to receiver (no shared memory).

### 3.2 Message Format

```rust
struct Message {
    sender:   u64,        // Task ID of sender (set by kernel)
    receiver: u64,        // Port ID of recipient
    data:     MessageData,
}

enum MessageData {
    Text(String),                          // Human-readable text
    Bytes(Vec<u8>),                        // Opaque binary
    Request { id: u64, data: Vec<u8> },    // RPC request
    Response { id: u64, data: Vec<u8> },   // RPC response
}
```

### 3.3 Port Lifecycle

| Operation | Syscall | Description |
|-----------|---------|-------------|
| Create port | `SYS_PORT_CREATE` (5) | Allocate a new port, returns port ID |
| Send message | `SYS_SEND` (6) | Enqueue message to target port |
| Receive | `SYS_RECEIVE` (7) | Dequeue oldest message (blocking) |

### 3.4 Request/Response Pattern

```
Task A                          Task B (service)
  │                                │
  │  Send(Request{id=42, data})    │
  │ ──────────────────────────────→│
  │                                │  process request
  │  Receive() ←───────────────────│  Send(Response{id=42, result})
  │  match id == 42                │
  │  done                          │
```

The `id` field correlates requests with responses. The kernel does not
interpret the `id` — it is a contract between sender and receiver.

### 3.5 Well-Known Ports

| Port ID | Owner | Purpose |
|---------|-------|---------|
| 0 | Kernel | Reserved (null port) |
| 1 | Kernel | Service discovery (future) |

---

## 4. Memory Layout

### 4.1 Virtual Address Space

```
0xFFFFFFFFFFFFFFFF ┌────────────────────┐
                   │                    │
                   │  Kernel Space      │  Ring 0
                   │  (higher-half)     │
0xFFFFFFFF80100000 ├────────────────────┤ ← Kernel .text
                   │  .text / .rodata   │
                   │  .data / .bss      │
                   │  Heap (100 KiB)    │
0xFFFFFFFF80000000 ├────────────────────┤ ← KERNEL_OFFSET
                   │                    │
                   │  (unmapped gap)    │
                   │                    │
0x00007FFFFFFFFFFF ├────────────────────┤ ← User stack top
                   │  User Stack        │  Ring 3
                   │  (grows down)      │
0x00007FFF00000000 ├────────────────────┤
                   │                    │
                   │  User Code / Data  │
                   │                    │
0x0000000000400000 ├────────────────────┤ ← User program base
                   │  (reserved / null) │
0x0000000000000000 └────────────────────┘
```

### 4.2 Kernel Memory Map

| Region | Virtual Address | Size | Permissions |
|--------|----------------|------|-------------|
| VGA text buffer | `0xB8000` | 4 KiB | RW (identity-mapped) |
| Kernel .text | `0xFFFFFFFF80100000` | ~64 KiB | RX |
| Kernel .data/.bss | `0xFFFFFFFF80110000` | ~16 KiB | RW |
| Kernel heap | `0x4444_4444_0000` | 100 KiB | RW |
| Kernel stack (RSP0) | TSS.rsp0 | 32 KiB | RW |
| IST[0] (double-fault) | TSS.ist[0] | 20 KiB | RW |

### 4.3 Page Table Flags

| Mapping | Present | Writable | User | NX |
|---------|---------|----------|------|-----|
| Kernel code | ✓ | ✗ | ✗ | ✗ |
| Kernel data | ✓ | ✓ | ✗ | ✓ |
| User code | ✓ | ✗ | ✓ | ✗ |
| User data | ✓ | ✓ | ✓ | ✓ |

---

## 5. Task Interface

### 5.1 Task Control Block

```rust
struct Task {
    id:       TaskId,    // Globally unique, monotonically increasing
    name:     String,    // Human-readable label ("idle", "fs_server")
    state:    TaskState, // Ready | Running | Blocked | Terminated
    priority: u8,        // Higher = more important (unused in round-robin)
}
```

### 5.2 Task States

```
                    ┌──────────┐
                    │  Ready   │ ← Created, waiting for CPU
                    └────┬─────┘
                         │ schedule()
                         ▼
                    ┌──────────┐
              ┌────→│ Running  │ ← On CPU
              │     └────┬─────┘
              │          │ block() / yield() / exit()
              │          ▼
              │     ┌──────────┐
              │     │ Blocked  │ ← Waiting for I/O or IPC
              │     └────┬─────┘
              │          │ wake()
              └──────────┘
                         │ exit()
                         ▼
                    ┌──────────┐
                    │Terminated│ ← Resources pending cleanup
                    └──────────┘
```

### 5.3 Scheduler

- **Algorithm:** Round-robin (FIFO queue)
- **Preemption:** Timer interrupt (future)
- **Quantum:** Configurable (default: 1 timer tick)
- **Idle task:** Always present in queue, priority 0, runs `hlt`

---

## 6. Driver Interface

### 6.1 Driver Registration

Drivers register interrupt handlers in the IDT during boot.

```rust
// In arch/x86_64/interrupts.rs:
idt[InterruptIndex::NewIrq.as_u8()].set_handler_fn(handler);
```

### 6.2 Interrupt Handler Contract

```rust
extern "x86-interrupt" fn handler(stack_frame: InterruptStackFrame) {
    // 1. Read data from hardware (port I/O or MMIO)
    // 2. Process the data
    // 3. Send EOI to PIC (mandatory)
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::NewIrq.as_u8());
    }
}
```

**Constraints:**
- Must not block (no sleeping, no waiting for user input)
- Must send EOI or the PIC will mask the IRQ line
- Must not acquire a spinlock already held by the interrupted context
  (use `interrupts::without_interrupts()` for VGA/serial locks)

### 6.3 Port I/O

```rust
use x86_64::instructions::port::Port;

let mut port = Port::new(0x60);  // I/O port address
let value: u8 = unsafe { port.read() };
unsafe { port.write(0xFF_u8) };
```

### 6.4 MMIO Access

```rust
use volatile::Volatile;

// All MMIO reads/writes must go through Volatile to prevent
// the compiler from optimizing away the access.
let value = mmio_ptr.read();
mmio_ptr.write(new_value);
```

---

## 7. Error Handling

### 7.1 Error Code Encoding

```
RAX = error_code | (1 << 63)
         │
         └── low 63 bits = error number
         bit 63 = error flag (always 1)
```

User-space checks: `if (rax & (1 << 63)) { /* error */ }`

### 7.2 Standard Error Codes

| Code | Name | Description |
|------|------|-------------|
| 1 | `EINVAL` | Invalid argument (null pointer, zero length) |
| 2 | `EAGAIN` | Resource temporarily unavailable |
| 3 | `EACCES` | Permission denied |
| 4 | `ENOSYS` | Syscall not implemented |
| 5 | `ESRCH` | Task/port not found |
| 6 | `ENOMEM` | Out of memory |
| 7 | `EIPC` | IPC delivery failure |

### 7.3 Kernel Panic

A kernel panic is an unrecoverable error. The kernel:
1. Prints `[PANIC] <message>` to VGA and serial
2. Halts all CPUs (`hlt` in an infinite loop)
3. Does not return

User-space should never trigger a kernel panic. All user errors are
reported via syscall error codes.

---

## Appendix: Syscall Number Table

| Number | Name | Arg 1 | Arg 2 | Arg 3 | Returns |
|--------|------|-------|-------|-------|---------|
| 0 | (reserved) | — | — | — | — |
| 1 | `WRITE` | `buf: *const u8` | `len: u64` | — | bytes written |
| 2 | `READ` | `buf: *mut u8` | `len: u64` | — | bytes read |
| 3 | `EXIT` | `status: u64` | — | — | (no return) |
| 4 | `YIELD` | — | — | — | `0` |
| 5 | `PORT_CREATE` | — | — | — | port id |
| 6 | `SEND` | `port: u64` | `msg: *const Message` | — | `0` |
| 7 | `RECEIVE` | `port: u64` | `buf: *mut Message` | — | `0` |

---

# 中文

## 1. 系统调用 ABI

### 1.1 调用约定

系统调用使用 `syscall` 指令（`x86_64` 上最快的用户→内核切换方式）。

| 寄存器 | `syscall` 入口时 | `sysretq` 返回时 |
|--------|-----------------|-----------------|
| `RAX` | 系统调用号 | 返回值 |
| `RDI` | 参数 1 | 保留 |
| `RSI` | 参数 2 | 保留 |
| `RDX` | 参数 3 | 保留 |
| `R10` | 参数 4（保留） | 保留 |
| `RCX` | *被 CPU 覆盖*（用户 RIP） | *被 CPU 覆盖* |
| `R11` | *被 CPU 覆盖*（用户 RFLAGS） | *被 CPU 覆盖* |

CPU 自动执行：
- 保存 `RIP → RCX`，`RFLAGS → R11`
- 从 STAR MSR 加载 `CS/SS`（内核段）
- 清除 `RFLAGS` 中的 `IF`（禁用中断）

### 1.2 返回约定

```
RAX = 结果                      （成功时）
RAX = 错误码 | (1 << 63)        （失败时，高位为 1）
```

`RAX` 的最高位作为错误标志。用户空间在解释返回值前必须检查第 63 位。

---

## 2. 系统调用参考

### 2.1 `SYS_WRITE` — 写入控制台

将字节写入内核的 VGA 控制台输出。

| 字段 | 值 |
|------|-----|
| 调用号 | `1` |
| 参数 1 | `buf` — 用户空间字节缓冲区指针 |
| 参数 2 | `len` — 要写入的字节数 |
| 返回值 | 已写入字节数，或错误码 |

### 2.2 `SYS_READ` — 读取输入

从键盘输入缓冲区读取字节到用户空间。

| 字段 | 值 |
|------|-----|
| 调用号 | `2` |
| 参数 1 | `buf` — 用户空间目标缓冲区指针 |
| 参数 2 | `len` — 最大读取字节数 |
| 返回值 | 实际读取字节数，或错误码 |

### 2.3 `SYS_EXIT` — 终止进程

终止调用进程。内核回收所有资源。

| 字段 | 值 |
|------|-----|
| 调用号 | `3` |
| 参数 1 | `status` — 退出码（0 = 成功） |
| 返回值 | 不返回 |

### 2.4 `SYS_YIELD` — 让出 CPU

主动将 CPU 让给调度器中的下一个任务。

| 字段 | 值 |
|------|-----|
| 调用号 | `4` |
| 返回值 | `0`（始终成功） |

---

## 3. IPC 协议

### 3.1 架构

每个任务创建一个或多个**端口**（邮箱）。消息按端口 ID 路由。内核将消息从发送方复制到接收方（无共享内存）。

### 3.2 消息格式

```rust
struct Message {
    sender:   u64,        // 发送方任务 ID（内核设置）
    receiver: u64,        // 接收方端口 ID
    data:     MessageData,
}

enum MessageData {
    Text(String),                          // 可读文本
    Bytes(Vec<u8>),                        // 二进制数据
    Request { id: u64, data: Vec<u8> },    // RPC 请求
    Response { id: u64, data: Vec<u8> },   // RPC 响应
}
```

### 3.3 请求/响应模式

`id` 字段关联请求与响应。内核不解释 `id` — 这是发送方和接收方之间的契约。

---

## 4. 内存布局

### 4.1 虚拟地址空间

| 区域 | 起始地址 | 大小 | 权限 |
|------|---------|------|------|
| 用户代码/数据 | `0x400000` | 可变 | RWX (Ring 3) |
| 用户栈 | `0x7FFF00000000` | 16 KiB | RW (Ring 3) |
| 内核 .text | `0xFFFFFFFF80100000` | ~64 KiB | RX (Ring 0) |
| 内核堆 | `0x4444_4444_0000` | 100 KiB | RW (Ring 0) |

### 4.2 页表标志

| 映射 | Present | Writable | User | NX |
|------|---------|----------|------|-----|
| 内核代码 | ✓ | ✗ | ✗ | ✗ |
| 内核数据 | ✓ | ✓ | ✗ | ✓ |
| 用户代码 | ✓ | ✗ | ✓ | ✗ |
| 用户数据 | ✓ | ✓ | ✓ | ✓ |

---

## 5. 任务接口

### 5.1 任务状态

- **Ready** — 已创建，等待 CPU
- **Running** — 正在 CPU 上执行
- **Blocked** — 等待 I/O 或 IPC
- **Terminated** — 资源待回收

### 5.2 调度器

- **算法：** 轮询（FIFO 队列）
- **抢占：** 定时器中断（未来）
- **空闲任务：** 始终存在于队列中，优先级 0，执行 `hlt`

---

## 6. 驱动接口

### 6.1 中断处理函数约束

- 不得阻塞（不得睡眠，不得等待用户输入）
- 必须发送 EOI，否则 PIC 将屏蔽 IRQ 线
- 不得获取被中断上下文已持有的自旋锁

---

## 附录：系统调用号表

| 编号 | 名称 | 参数 1 | 参数 2 | 参数 3 | 返回值 |
|------|------|--------|--------|--------|--------|
| 1 | `WRITE` | `buf: *const u8` | `len: u64` | — | 已写入字节数 |
| 2 | `READ` | `buf: *mut u8` | `len: u64` | — | 已读取字节数 |
| 3 | `EXIT` | `status: u64` | — | — | （不返回） |
| 4 | `YIELD` | — | — | — | `0` |
| 5 | `PORT_CREATE` | — | — | — | 端口 ID |
| 6 | `SEND` | `port: u64` | `msg: *const Message` | — | `0` |
| 7 | `RECEIVE` | `port: u64` | `buf: *mut Message` | — | `0` |
