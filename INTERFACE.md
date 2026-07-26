# OpenOS System Interface

This document defines the complete system interface for the OpenOS microkernel.
Both kernel (`kernel/src/syscall/`) and user-space (`sdk/src/lib.rs`) must
use the numbers and conventions defined here.

**Source of truth**: `kernel/src/syscall/number.rs` defines all syscall numbers.
This document is the human-readable reference.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Syscall Table](#2-syscall-table)
3. [Error Codes](#3-error-codes)
4. [IPC Protocol](#4-ipc-protocol)
5. [Capability Model](#5-capability-model)
6. [Signal Handling](#6-signal-handling)
7. [Memory Management](#7-memory-management)
8. [Process Model](#8-process-model)
9. [Filesystem](#9-filesystem)
10. [Appendix](#10-appendix)

---

## 1. Overview

OpenOS is a bare-metal x86_64 microkernel. User-space interacts with the kernel
exclusively through the `syscall` instruction. There are no POSIX compatibility
layers; all interfaces are native OpenOS designs.

### Calling Convention

User-space invokes a syscall via the x86_64 `syscall` instruction.

| Position | Register | Name |
|----------|----------|------|
| number   | `rax`    | Syscall number |
| arg0     | `rdi`    | First argument |
| arg1     | `rsi`    | Second argument |
| arg2     | `rdx`    | Third argument |
| arg3     | `r10`    | Fourth argument |
| arg4     | `r8`     | Fifth argument |
| arg5     | `r9`     | Sixth argument |

**Return value**: `rax` (i64).

- Positive (or zero) = success value
- Negative = error code (see [Error Codes](#3-error-codes))

**Clobbered registers**: `rcx` and `r11` are destroyed by the `syscall`
instruction (Intel/AMD ABI).

**Maximum message size**: 4096 bytes. Pointers to user-space buffers are
validated to be non-null, below `USER_SPACE_MAX` (128 TiB), and within bounds.

**Preemption**: On each syscall entry, the handler checks `NEED_RESCHEDULE`
(set by the timer interrupt). If set, the current task is preempted before the
syscall is dispatched. This provides cooperative preemption on syscall boundaries.

---

## 2. Syscall Table

### 2.1 Channel (IPC) — `0x01`-`0x05`

| Number | Name               | Arguments                                    | Return                  | Description |
|--------|--------------------|----------------------------------------------|-------------------------|-------------|
| `0x01` | `channel_create`   | —                                            | handle (End A)          | Create a new IPC channel. Both endpoints installed in caller's handle table. |
| `0x02` | `channel_send`     | handle, msg_ptr, msg_len                     | 0                       | Send a message. Blocks if peer has not yet called `receive`. |
| `0x03` | `channel_receive`  | handle, buf_ptr, buf_len                     | bytes received          | Receive a message. Blocks if no message pending. |
| `0x04` | `channel_call`     | handle, req_ptr, req_len, reply_ptr, _reply_len | reply bytes         | Atomic send-and-wait-for-reply (RPC primitive). |
| `0x05` | `channel_reply`    | handle, reply_ptr, reply_len                 | 0                       | Reply to a pending `channel_call`. Wakes the caller. |

### 2.2 Handle — `0x10`-`0x12`

| Number | Name               | Arguments              | Return          | Description |
|--------|--------------------|------------------------|-----------------|-------------|
| `0x10` | `handle_close`     | handle                 | 0               | Close a handle. Drops kernel object if last reference. |
| `0x11` | `handle_duplicate` | handle, new_rights     | new handle      | Duplicate with optionally narrowed rights (monotonic reduction). |
| `0x12` | `handle_transfer`  | handle, channel_handle, rights | 0        | Transfer a handle to another task via a channel. |

### 2.3 Process + Memory + Time — `0x30`-`0x3E`

| Number | Name               | Arguments                       | Return              | Description |
|--------|--------------------|---------------------------------|---------------------|-------------|
| `0x30` | `process_create`   | _job, name_ptr, name_len        | task_id             | Create a new process in the scheduler. |
| `0x31` | `process_start`    | task_id, elf_name_ptr, name_len | task_id             | Load ELF from initrd/disk into the process's address space. |
| `0x32` | `process_exit`     | status                          | (does not return)   | Terminate the current process. Closes all handles. |
| `0x33` | `process_wait`     | child_id, timeout_ticks         | exit status         | Wait for a child to exit. `timeout=0` is non-blocking, `u64::MAX` blocks forever. |
| `0x34` | `brk`              | new_brk                         | break address       | Set/query heap end. `0` returns current break. |
| `0x35` | `mmap`             | hint, size, flags               | virtual address     | Map anonymous memory. `flags`: bit 0=writable, bit 1=executable. |
| `0x36` | `munmap`           | virt_addr, size                 | 0                   | Unmap a memory region. |
| `0x37` | `getpid`           | —                               | task_id             | Get the current process ID. |
| `0x38` | `getppid`          | —                               | parent task_id      | Get the parent process ID. Returns 0 if no parent. |
| `0x3D` | `list_tasks`       | buf_ptr, buf_len                | task count          | Serialize all task info (40 bytes/entry) into buffer. |
| `0x3E` | `clock_gettime`    | clock_id, timespec_ptr          | 0                   | Get monotonic time. Writes `{sec: u64, nsec: u64}` to buffer. `clock_id=0` only. |

### 2.4 Thread — `0x40`-`0x42`

| Number | Name               | Arguments                    | Return          | Description |
|--------|--------------------|------------------------------|-----------------|-------------|
| `0x40` | `thread_create`    | entry_point, stack_ptr, arg  | new task_id     | Create a user-mode thread sharing parent's page table. |
| `0x41` | `thread_exit`      | —                            | (does not return)| Terminate current thread (status 0). |
| `0x42` | `thread_yield`     | —                            | 0               | Yield CPU to next ready task. |

### 2.5 Pipe — `0x43`

| Number | Name               | Arguments       | Return | Description |
|--------|--------------------|-----------------|--------|-------------|
| `0x43` | `pipe`             | fds_ptr         | 0      | Create pipe pair. Writes `[read_fd, write_fd]` to user buffer. |

### 2.6 Signal — `0x44`-`0x4A`

| Number | Name               | Arguments              | Return          | Description |
|--------|--------------------|------------------------|-----------------|-------------|
| `0x44` | `kill`             | pid, sig               | 0               | Send signal to a task. Wakes target if blocked. |
| `0x45` | `signal`           | sig, handler           | previous handler| Set signal handler. `SIG_DFL=0`, `SIG_IGN=1`, else=user addr. |
| `0x46` | `sigreturn`        | frame_ptr              | 0               | Return from signal handler. Restores saved context. |
| `0x4A` | `sigprocmask`      | how, set_ptr, oldset_ptr | 0             | Get/set signal mask. `how`: `SIG_BLOCK=0`, `SIG_UNBLOCK=1`, `SIG_SETMASK=2`. |

### 2.7 Memory Protection — `0x4B`

| Number | Name               | Arguments              | Return | Description |
|--------|--------------------|------------------------|--------|-------------|
| `0x4B` | `mprotect`         | addr, size, prot       | 0      | Change protection on mmap'd pages (reserved, not yet dispatched). |

### 2.8 Dup2 / Environment / Working Directory — `0x47`-`0x49`, `0xCD`-`0xCE`

| Number | Name               | Arguments                    | Return          | Description |
|--------|--------------------|------------------------------|-----------------|-------------|
| `0x47` | `dup2`             | old_fd, new_fd               | new_fd          | Duplicate file descriptor. Overwrites new_fd if open. |
| `0x48` | `env_get`          | key_ptr, key_len, val_ptr, val_len | value length | Get environment variable. Returns 0 if key absent. |
| `0x49` | `env_set`          | key_ptr, key_len, val_ptr, val_len | 0            | Set environment variable. Overwrites if exists. |
| `0xCD` | `chdir`            | path_ptr, path_len           | 0               | Change working directory. Validates path exists and is a directory. |
| `0xCE` | `getcwd`           | buf_ptr, buf_len             | cwd length      | Get current working directory. |

### 2.9 Socket / DNS — `0xA0`-`0xA8`

| Number | Name               | Arguments                          | Return          | Description |
|--------|--------------------|------------------------------------|-----------------|-------------|
| `0xA0` | `socket`           | socket_type                        | socket fd       | Create socket. `0`=TCP, `1`=UDP, `2`=Raw. |
| `0xA1` | `bind`             | sock_fd, addr, port                | 0               | Bind socket to local address/port. |
| `0xA2` | `listen`           | sock_fd                            | 0               | Mark TCP socket as listening. Registers port in kernel listen table. |
| `0xA3` | `accept`           | sock_fd                            | new sock_fd     | Accept incoming TCP connection. Blocks until one arrives. |
| `0xA4` | `connect`          | sock_fd, addr, port                | 0               | Connect to remote TCP endpoint. |
| `0xA5` | `sendto`           | sock_fd, buf_ptr, len, addr, port  | bytes sent      | Send data on connected TCP socket. |
| `0xA6` | `recvfrom`         | sock_fd, buf_ptr, buf_len          | bytes received  | Receive data from socket (non-blocking). |
| `0xA7` | `close_sock`       | sock_fd                            | 0               | Close socket. Initiates TCP FIN if connected. |
| `0xA8` | `dns_resolve`      | name_ptr, name_len, out_ptr        | 0               | Resolve hostname to IPv4 address (4 bytes written to out_ptr). |

### 2.10 Hardware Access (User-Space Drivers) — `0xB0`-`0xB4`

| Number | Name               | Arguments              | Return          | Description |
|--------|--------------------|------------------------|-----------------|-------------|
| `0xB0` | `port_in`          | port, size             | value read      | Read from I/O port. Size: 1, 2, or 4 bytes. Ports < `0x60` blocked. |
| `0xB1` | `port_out`         | port, value, size      | 0               | Write to I/O port. Ports < `0x60` blocked. |
| `0xB2` | `mmio_map`         | phys_addr, size        | virtual address | Map physical MMIO region into user address space. Uncached. |
| `0xB3` | `mmio_unmap`       | virt_addr, size        | 0               | Unmap a previously mapped MMIO region. |
| `0xB4` | `irq_wait`         | handle, blocking       | device data     | Wait for hardware IRQ. Returns device data byte. Blocking if `blocking != 0`. |

### 2.11 Filesystem Metadata — `0xC0`-`0xCB`

| Number | Name               | Arguments                          | Return          | Description |
|--------|--------------------|------------------------------------|-----------------|-------------|
| `0xC0` | `fs_unlink`        | path_ptr, path_len                 | 0               | Delete a file. |
| `0xC1` | `fs_rename`        | old_ptr, old_len, new_ptr, new_len | 0               | Rename (copy + delete). |
| `0xC2` | `fs_mkdir`         | path_ptr, path_len, perms          | 0               | Create a directory. |
| `0xC3` | `fs_rmdir`         | path_ptr, path_len                 | 0               | Remove a directory. |
| `0xC4` | `fs_stat`          | path_ptr, path_len, out_ptr        | 0               | Get file metadata. Writes `{size: u64, ino: u64, mode: u32}` (20 bytes). |
| `0xC5` | `fs_readdir`       | path_ptr, path_len, out_ptr        | bytes written   | Read directory entries (null-terminated names). |
| `0xC6` | `getdents64`       | fd, buf_ptr, buf_len               | bytes written   | Read directory entries in `linux_dirent64` format. |
| `0xC7` | `fstat`            | fd, out_ptr                        | 0               | Get file status by file descriptor. |
| `0xC8` | `lstat`            | path_ptr, path_len, out_ptr        | 0               | Get file status without following symlinks. Currently delegates to `fs_stat`. |
| `0xC9` | `access`           | path_ptr, path_len, mode           | 0               | Check file accessibility. Mode: `F_OK=0`, `R_OK=1`, `W_OK=2`, `X_OK=4`. |
| `0xCA` | `symlink`          | target_ptr, target_len, link_ptr, link_len | 0      | Create a symbolic link. |
| `0xCB` | `readlink`         | path_ptr, path_len, buf_ptr, buf_len | bytes written | Read symlink target. |

### 2.12 OpenOS-Specific — `0xF0`-`0xFF`

| Number | Name               | Arguments              | Return          | Description |
|--------|--------------------|------------------------|-----------------|-------------|
| `0xF0` | `console_write`    | ptr, len               | bytes written   | Write to kernel debug console (serial port). |
| `0xF1` | `sleep`            | ticks                  | 0               | Sleep for N timer ticks. Yields CPU. |
| `0xF2` | `event_create`     | —                      | handle          | Create an event object. |
| `0xF3` | `event_signal`     | handle                 | 0               | Signal an event. Wakes any waiter. |
| `0xF4` | `console_read`     | buf_ptr, buf_len, flags| bytes read      | Read from keyboard input buffer. `flags & 1` = blocking. |
| `0xF5` | `endpoint_register`| name_ptr, name_len, handle | 0          | Register named service endpoint in task namespace. |
| `0xF6` | `endpoint_discover`| name_ptr, name_len     | handle          | Discover named service endpoint. |
| `0xF7` | `fs_open`          | name_ptr, name_len, flags | fd           | Open file. `flags=0` read-only, `flags=1` write/create/truncate. |
| `0xF8` | `fs_read`          | fd, buf_ptr, buf_len   | bytes read      | Read from file descriptor. Also supports pipes. |
| `0xF9` | `fs_write`         | fd, data_ptr, data_len | bytes written   | Write to file descriptor. `fd=1` redirects to serial console. |
| `0xFA` | `fs_close`         | fd                     | 0               | Close file descriptor. |
| `0xFB` | `event_wait`       | handle                 | 0               | Block until event is signaled. Clears signal on return. |
| `0xFC` | `event_destroy`    | handle                 | 0               | Destroy event (close handle). |
| `0xFD` | `net_send`         | data_ptr, data_len     | bytes sent      | Send raw Ethernet frame. |
| `0xFE` | `net_receive`      | buf_ptr, buf_len       | bytes received  | Receive raw Ethernet frame (non-blocking). |
| `0xFF` | `fs_seek`          | fd, offset, whence     | new offset      | Seek in file. `SEEK_SET=0`, `SEEK_CUR=1`, `SEEK_END=2`. |

---

## 3. Error Codes

Error codes are returned as negative values in `rax`. The `Error` enum defines
the canonical error codes:

| Code  | Name               | Description |
|-------|--------------------|-------------|
| `-1`  | `InvalidArgument`  | Argument was invalid or out of range. |
| `-2`  | `NotFound`         | Requested resource was not found. |
| `-3`  | `PermissionDenied` | Caller lacks required rights. |
| `-4`  | `OutOfMemory`      | Kernel is out of memory. |
| `-5`  | `Busy`             | Resource is busy (e.g., port in use). |
| `-6`  | `ChannelClosed`    | Channel has been closed by the peer. |
| `-7`  | `WouldBlock`       | Operation would block (non-blocking mode). |
| `-8`  | `Timeout`          | Operation timed out. |
| `-9`  | `BadPointer`       | User-space pointer is invalid or in kernel space. |
| `-10` | `UnknownSyscall`   | Syscall number is not recognized. |
| `-32` | `EPIPE`            | Write to pipe with no reader (internal, returned by pipe subsystem). |

---

## 4. IPC Protocol

### 4.1 Channels

A Channel is a bidirectional, synchronous message pipe with two endpoints (A and B).
Messages are byte sequences up to **4096 bytes**. Channels are stored in per-task
handle tables as `KernelObject::ChannelEndA` and `KernelObject::ChannelEndB`.

#### Operations

1. **`send(from, msg)`** — Send a message from one end to the other.
   - If the peer is blocked in `receive`, the message is delivered immediately
     and the peer is woken (`SendResult::Delivered`).
   - Otherwise, the message is stored and `SendResult::Pending` is returned.
     The sender is registered as blocked until the peer calls `receive`.

2. **`receive(on)`** — Receive a message on one end.
   - If a message is pending, returns `GotMessage(msg, sender_id)`.
   - Otherwise, blocks the caller until a message arrives.

3. **`call(from, msg)`** — Atomic send-and-wait-for-reply (RPC primitive).
   - Sends the message, then blocks until the peer calls `reply`.
   - If a reply is already pending from a previous `reply`, returns it immediately.

4. **`reply(on, msg)`** — Reply to a pending `call`.
   - Stores the reply and unblocks the caller.
   - If no caller is waiting, stores the reply for later consumption.

#### Blocking Semantics

All blocking is cooperative: the syscall handler captures the current register
context and calls `block_and_switch` to yield the CPU. The blocked task is moved
to the scheduler's blocked queue and woken when the peer performs the matching
operation. When no other task is ready, the kernel falls back to HLT spin-wait.

#### Handle Transfer

Handles can be transferred between tasks via a channel using `handle_transfer`.
Transferred handles are stored in the channel's pending handles list and prepended
to the next message received. Up to **64** pending handles per channel.

### 4.2 Pipes

A pipe is a unidirectional byte stream with a **4 KiB** ring buffer. Created via
`SYS_PIPE` (`0x43`), which returns two file descriptors `[read_fd, write_fd]`.

| Condition                  | Behavior |
|----------------------------|----------|
| Read on empty pipe         | Returns 0 bytes (EOF if writer dropped). |
| Write on full pipe         | Returns 0 bytes written (partial write if some space available). |
| Write with reader dropped  | Returns `EPIPE` (`-32`). |
| Read with writer dropped   | Returns 0 (EOF) after draining buffered data. |

Pipe file descriptors are registered in the task's fd table and work with the
standard `fs_read`/`fs_write` syscalls.

---

## 5. Capability Model

### 5.1 Handle Bit Layout

Handles are 64-bit opaque tokens. User-space cannot construct valid handles
without the kernel providing one.

```
 63                38 37          28 27                    0
+--------------------+--------------+------------------------+
| generation (26)    | rights (10)  |      slot_id (28)     |
+--------------------+--------------+------------------------+
```

| Field        | Bits   | Width | Description |
|-------------|--------|-------|-------------|
| `slot_id`    | 0-27   | 28    | Index into task's HandleTable. Supports up to 268M handles. |
| `rights`     | 28-37  | 10    | Capability rights bitmask (see below). |
| `generation` | 38-63  | 26    | Generation counter. Prevents use-after-close. |

### 5.2 Rights Flags

| Bit | Name         | Description |
|-----|-------------|-------------|
| 0   | `READ`       | Read from the object. |
| 1   | `WRITE`      | Write to the object. |
| 2   | `EXECUTE`    | Execute (e.g., start a process). |
| 3   | `TRANSFER`   | Transfer handle to another task via channel. |
| 4   | `DUPLICATE`  | Duplicate the handle. |
| 5   | `SIGNAL`     | Signal an event. |
| 6   | `WAIT`       | Wait on an event or process exit. |
| 7   | `DESTROY`    | Close/destroy the object. |
| 8   | `MAP`        | Map into address space (MMIO). |
| 9   | `CONFIGURE`  | Configure object properties. |

Rights can only be narrowed (via `intersect`), never amplified. `handle_duplicate`
intersects the requested rights with the original handle's rights.

### 5.3 Kernel Object Types

| Variant       | Description |
|--------------|-------------|
| `ChannelEndA` | End A of a channel (client end). |
| `ChannelEndB` | End B of a channel (server end). |
| `Event`       | One-shot or level-triggered event signal. |
| `IrqEvent`    | IRQ-backed event, signaled by hardware interrupt handler. |
| `File`        | Open file in the VFS. |
| `PipeReader`  | Read end of a pipe. |
| `PipeWriter`  | Write end of a pipe. |

### 5.4 Security Properties

- **Unforgeable**: User-space cannot construct a valid handle. The generation
  counter makes brute-force guessing impractical (2^26 possible generations).
- **Monotonic reduction**: Rights can only be narrowed, never amplified.
- **Per-task isolation**: Handle tables are per-task. Handles cannot be used
  across tasks without explicit transfer.

---

## 6. Signal Handling

### 6.1 Signal Numbers

Signals are identified by integers 1..31. The following signals are defined:

| Number | Name      | Default Action | Description |
|--------|-----------|----------------|-------------|
| 1      | `SIGHUP`  | Terminate      | Hangup detected on controlling terminal. |
| 2      | `SIGINT`  | Terminate      | Interrupt from keyboard (Ctrl-C). |
| 3      | `SIGQUIT` | Terminate      | Quit from keyboard (Ctrl-\\). |
| 4      | `SIGILL`  | Terminate      | Illegal instruction. |
| 5      | `SIGTRAP` | Terminate      | Trace/breakpoint trap. |
| 6      | `SIGABRT` | Terminate      | Abort signal. |
| 7      | `SIGBUS`  | Terminate      | Bus error (bad memory access). |
| 8      | `SIGFPE`  | Terminate      | Floating-point exception. |
| 9      | `SIGKILL` | Terminate      | Kill signal. **Cannot be caught or ignored.** |
| 10     | `SIGUSR1` | Terminate      | User-defined signal 1. |
| 11     | `SIGSEGV` | Terminate      | Invalid memory reference (segmentation fault). |
| 12     | `SIGUSR2` | Terminate      | User-defined signal 2. |
| 13     | `SIGPIPE` | Terminate      | Broken pipe: write to pipe with no readers. |
| 14     | `SIGALRM` | Terminate      | Timer signal. |
| 15     | `SIGTERM` | Terminate      | Termination signal. |
| 17     | `SIGCHLD` | Ignore         | Child stopped or terminated. |
| 16,18-31 | (reserved) | Terminate   | Available for future use. |

### 6.2 Handler Conventions

| Value | Meaning |
|-------|---------|
| `0` (`SIG_DFL`) | Kernel default action (terminate for most signals). |
| `1` (`SIG_IGN`) | Ignore the signal — silently discarded. |
| Other            | User-space handler address. |

`SIGKILL` cannot have its handler changed — attempting to set a handler returns
`PermissionDenied`.

### 6.3 Signal Delivery Mechanism

Signals are tracked per-task using a `SignalState` structure:

- **`pending`** (u64 bitmask) — bit N is set when signal N is pending.
- **`blocked`** (u64 bitmask) — bit N is set when signal N is blocked.
- **`handlers`** (u64 array, index 0-31) — per-signal handler addresses.

When `SYS_KILL` is called, the target task's pending bit is set. The signal is
delivered on the target's next syscall entry or context switch.

During delivery, the kernel:

1. Pushes a `SignalFrame` onto the user stack:

   ```
   +---------------------+  <- RSP after signal delivery
   |  saved_r11 (RFLAGS) |
   |  saved_rcx (RIP)    |
   |  saved_rsp          |
   |  saved_rdi          |
   |  sig_num            |
   |  ret_addr           |  (address of sigreturn trampoline)
   +---------------------+
   ```

2. Sets RDI to the signal number (handler argument).
3. Sets RIP to the handler address.
4. The handler runs and eventually calls `SYS_SIGRETURN`.
5. `sigreturn` restores the saved context from the `SignalFrame`.

Signals are dequeued in lowest-number-first order. Blocked signals remain pending
but are not delivered until unblocked.

### 6.4 Signal Mask (`sigprocmask`)

| Operation       | Value | Effect |
|----------------|-------|--------|
| `SIG_BLOCK`     | 0     | Add signals to blocked mask. |
| `SIG_UNBLOCK`   | 1     | Remove signals from blocked mask. |
| `SIG_SETMASK`   | 2     | Set blocked mask to exact value. |

---

## 7. Memory Management

### 7.1 Address Space Layout

```
0x0000_0000_0000_0000  +---------------------------+
                       | User space (128 TiB max)  |
                       | - Code, data, heap, stack |
                       | - mmap regions            |
                       | - MMIO mappings           |
0x0000_8000_0000_0000  +---------------------------+  USER_SPACE_MAX
                       | Non-canonical hole        |
0xFFFF_8000_0000_0000  +---------------------------+
                       | Kernel space (higher half) |
                       | - Kernel code and data    |
                       | - Physical memory mapping |
0xFFFF_FFFF_FFFF_FFFF  +---------------------------+
```

User-space pointers must be below `USER_SPACE_MAX` (`0x0000_8000_0000_0000`, 128 TiB).
The kernel validates all user-space pointers on every syscall.

### 7.2 `brk` (0x34)

Sets the program break (heap end). The kernel allocates/frees pages as needed.

- `brk(0)` — returns the current break address.
- `brk(addr)` — sets the break to `addr` (page-aligned). Returns the new break.

The heap grows upward from the initial break set by the ELF loader. A `VmaRegion`
of type `Heap` tracks the heap area.

### 7.3 `mmap` (0x35)

Maps anonymous memory into the user address space.

- `hint=0` — kernel chooses the virtual address (scans from `0x4000_0000` upward).
- `hint!=0` — preferred address (may be relocated if occupied).
- `flags` bit 0 — writable.
- `flags` bit 1 — executable.
- Pages are zero-initialized.
- Each mapped region is tracked as a `VmaRegion` of type `Mmap`.

Returns the mapped virtual address, or a negative error code.

### 7.4 `munmap` (0x36)

Unmaps a previously mapped region. Address must be page-aligned. Physical frames
are freed, the page table entries are cleared, and the VMA entry is removed.

### 7.5 `mprotect` (0x4B)

Changes memory protection on mmap'd pages. The syscall number is reserved but
not yet dispatched in the handler.

### 7.6 MMIO Mapping (`mmio_map` / `mmio_unmap`)

Maps physical device memory (MMIO) into user-space for user-space drivers.
Both address and size must be page-aligned. Pages are mapped uncached
(`NO_CACHE` flag) and non-executable (`NO_EXECUTE`). The kernel scans from
high user-space addresses downward to find free virtual address ranges.

### 7.7 Page Table

Each user process has its own P4 page table. The kernel's higher-half entries
(indices 256-511) are copied from the kernel page table during process creation.
The lower half (user space) is populated by the ELF loader and mmap.

Page table flags used:

| Flag              | Meaning |
|------------------|---------|
| `PRESENT`         | Page is mapped. |
| `WRITABLE`        | Page is writable. |
| `USER_ACCESSIBLE` | Page is accessible from Ring 3. |
| `NO_EXECUTE`      | Page is not executable (NX bit). |
| `NO_CACHE`        | Uncached (used for MMIO). |

---

## 8. Process Model

### 8.1 Task Lifecycle

```
process_create  -->  [Created]  -->  process_start  -->  [Ready]
                                                        |
                                                   scheduler picks
                                                        |
                                                    [Running]
                                                   /    |    \
                                     thread_yield  block  process_exit
                                          |          |         |
                                       [Ready]   [Blocked]  [Terminated]
                                                    |
                                                 wake event
                                                    |
                                                 [Ready]
```

### 8.2 Task States

| State        | Value | Description |
|-------------|-------|-------------|
| `Ready`      | 0     | In the scheduler's ready queue, waiting for CPU. |
| `Running`    | 1     | Currently executing on a CPU. |
| `Blocked`    | 2     | Waiting for an event (IPC, sleep, IRQ, etc.). |
| `Terminated` | 3     | Exited. Exit status available for `process_wait`. |

### 8.3 Task Entry Layout (40 bytes)

Serialized by `list_tasks`:

| Offset | Size | Field      | Description |
|--------|------|-----------|-------------|
| 0      | 8    | task_id    | u64 LE |
| 8      | 1    | state      | 0=Ready, 1=Running, 2=Blocked, 3=Terminated |
| 9      | 1    | priority   | u8 |
| 10     | 2    | reserved   | Zero |
| 12     | 32   | name       | UTF-8, zero-padded |

### 8.4 Scheduling

OpenOS uses a **SMP round-robin** scheduler with per-CPU ready queues and work
stealing. The timer interrupt sets `NEED_RESCHEDULE`, which is checked on the next
syscall entry. This provides cooperative preemption on syscall boundaries.

- Each CPU has its own ready queue.
- New tasks are enqueued on the least-loaded CPU.
- Work stealing balances load across CPUs.
- IPI (inter-processor interrupt) wakes remote CPUs when needed.

### 8.5 Saved Context Layout (136 bytes)

The `SavedContext` structure preserves register state across context switches:

| Offset | Field       | Purpose |
|--------|------------|---------|
| 0      | `r9`       | arg5 |
| 8      | `r8`       | arg4 |
| 16     | `rdx`      | arg3 |
| 24     | `rsi`      | arg2 |
| 32     | `rdi`      | arg1 |
| 40     | `rax`      | syscall number / return |
| 48     | `r15`      | callee-saved |
| 56     | `r14`      | callee-saved |
| 64     | `r13`      | callee-saved |
| 72     | `r12`      | callee-saved |
| 80     | `rbx`      | callee-saved |
| 88     | `rbp`      | callee-saved |
| 96     | `r11`      | RFLAGS (from SYSCALL) |
| 104    | `rcx`      | RIP (from SYSCALL) |
| 112    | `rsp`      | stack pointer |
| 120    | `is_kernel`| 1=kernel (IRETQ), 0=user (SYSRET) |
| 128    | `cr3`      | page table physical address |

### 8.6 Process Creation

A two-step process:

1. `process_create` — allocates a task in the scheduler, returns `task_id`.
2. `process_start` — loads an ELF binary (from initrd or disk) into the task's
   address space, sets up the entry point and stack.

The child process gets its own page table (kernel entries shared, user entries fresh).
The child's `parent_id` is set to the creator's task ID so `getppid` works.

### 8.7 Thread Creation

`thread_create` creates a new task sharing the parent's page table. The new thread
starts executing at the given entry point with the given stack pointer. The argument
is passed in RDI (first argument register). The thread's `parent_id` is set so
the scheduler can wake the parent on exit.

---

## 9. Filesystem

### 9.1 VFS Interface

The VFS provides a unified interface over multiple filesystem implementations:

- **ramfs** — mounted at `/` (in-memory filesystem)
- **ext2** — mounted at `/disk` (block device filesystem via VirtIO-Block)

Path resolution uses `resolve_fs` to find the appropriate filesystem. Paths starting
with `/disk` are routed to the ext2 filesystem; all others go to ramfs.

Relative paths are resolved against the task's current working directory (`cwd`).

### 9.2 Mount Table

| Mount Point | Filesystem | Backing |
|------------|------------|---------|
| `/`         | ramfs      | In-memory |
| `/disk`     | ext2       | VirtIO-Block device |

### 9.3 File Descriptor Table

Each task maintains a file descriptor table (`BTreeMap<u64, FdEntry>`):

| FD    | Description |
|-------|-------------|
| 0     | `stdin` (reserved, not backed by VFS) |
| 1     | `stdout` (reserved, redirects to serial console) |
| 2-1023 | VFS files and pipes |

Each `FdEntry` contains:

- `path` — full filesystem path
- `ino` — inode number (or handle value for pipes)
- `offset` — current read/write position

The maximum file descriptor number is **1023** (`FD_LIMIT = 1024`).

### 9.4 Stat Buffer Layout (20 bytes)

Written by `fs_stat`, `fstat`, and `lstat`:

| Offset | Size | Field  | Description |
|--------|------|--------|-------------|
| 0      | 8    | size   | File size in bytes (u64 LE) |
| 8      | 8    | ino    | Inode number (u64 LE) |
| 16     | 4    | mode   | File mode (u32 LE): `0o40755` for directories, `0o100644` for files |

### 9.5 linux_dirent64 Layout

Written by `getdents64`:

| Offset | Size | Field    | Description |
|--------|------|----------|-------------|
| 0      | 8    | d_ino    | Inode number (u64 LE) |
| 8      | 8    | d_off    | Offset to next entry (u64 LE) |
| 16     | 2    | d_reclen | Record length, 8-byte aligned (u16 LE) |
| 18     | 1    | d_type   | File type: 4=directory, 8=regular file |
| 19     | N    | d_name   | Null-terminated filename |

### 9.6 Seek Constants

| Constant    | Value | Description |
|------------|-------|-------------|
| `SEEK_SET`  | 0     | Offset from beginning of file. |
| `SEEK_CUR`  | 1     | Offset from current position. |
| `SEEK_END`  | 2     | Offset from end of file. |

### 9.7 Access Mode Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `F_OK`   | 0     | Test for existence. |
| `R_OK`   | 1     | Test for read permission. |
| `W_OK`   | 2     | Test for write permission. |
| `X_OK`   | 4     | Test for execute permission. |

---

## 10. Appendix

### A. Syscall Number Ranges

| Range         | Subsystem               | Count |
|--------------|-------------------------|-------|
| `0x01-0x0F`  | Channel (IPC)           | 5     |
| `0x10-0x1F`  | Handle                  | 3     |
| `0x20-0x2F`  | (reserved)              | —     |
| `0x30-0x3F`  | Process + Memory + Time | 11    |
| `0x40-0x4F`  | Thread + Pipe + Signal  | 10    |
| `0x50-0x9F`  | (reserved)              | —     |
| `0xA0-0xAF`  | Socket + DNS            | 9     |
| `0xB0-0xBF`  | Hardware access         | 5     |
| `0xC0-0xCF`  | Filesystem metadata     | 13    |
| `0xD0-0xEF`  | (reserved)              | —     |
| `0xF0-0xFF`  | OpenOS-specific         | ~14   |

**Total**: 67 syscalls across 12 subsystems.

### B. Syscall Number Assignments

| Number | Name | Number | Name | Number | Name |
|--------|------|--------|------|--------|------|
| `0x01` | channel_create | `0x02` | channel_send | `0x03` | channel_receive |
| `0x04` | channel_call | `0x05` | channel_reply | `0x10` | handle_close |
| `0x11` | handle_duplicate | `0x12` | handle_transfer | `0x30` | process_create |
| `0x31` | process_start | `0x32` | process_exit | `0x33` | process_wait |
| `0x34` | brk | `0x35` | mmap | `0x36` | munmap |
| `0x37` | getpid | `0x38` | getppid | `0x3D` | list_tasks |
| `0x3E` | clock_gettime | `0x40` | thread_create | `0x41` | thread_exit |
| `0x42` | thread_yield | `0x43` | pipe | `0x44` | kill |
| `0x45` | signal | `0x46` | sigreturn | `0x47` | dup2 |
| `0x48` | env_get | `0x49` | env_set | `0x4A` | sigprocmask |
| `0x4B` | mprotect | `0xA0` | socket | `0xA1` | bind |
| `0xA2` | listen | `0xA3` | accept | `0xA4` | connect |
| `0xA5` | sendto | `0xA6` | recvfrom | `0xA7` | close_sock |
| `0xA8` | dns_resolve | `0xB0` | port_in | `0xB1` | port_out |
| `0xB2` | mmio_map | `0xB3` | mmio_unmap | `0xB4` | irq_wait |
| `0xC0` | fs_unlink | `0xC1` | fs_rename | `0xC2` | fs_mkdir |
| `0xC3` | fs_rmdir | `0xC4` | fs_stat | `0xC5` | fs_readdir |
| `0xC6` | getdents64 | `0xC7` | fstat | `0xC8` | lstat |
| `0xC9` | access | `0xCA` | symlink | `0xCB` | readlink |
| `0xCD` | chdir | `0xCE` | getcwd | `0xF0` | console_write |
| `0xF1` | sleep | `0xF2` | event_create | `0xF3` | event_signal |
| `0xF4` | console_read | `0xF5` | endpoint_register | `0xF6` | endpoint_discover |
| `0xF7` | fs_open | `0xF8` | fs_read | `0xF9` | fs_write |
| `0xFA` | fs_close | `0xFB` | event_wait | `0xFC` | event_destroy |
| `0xFD` | net_send | `0xFE` | net_receive | `0xFF` | fs_seek |

### C. Constants Reference

| Constant | Value | Description |
|----------|-------|-------------|
| `USER_SPACE_MAX` | `0x0000_8000_0000_0000` | 128 TiB — upper bound of user virtual addresses. |
| `MAX_MSG_SIZE` | `4096` | Maximum bytes per user-kernel data copy. |
| `PAGE_SIZE` | `4096` | Page size (4 KiB). |
| `FD_STDIN` | `0` | Standard input file descriptor. |
| `FD_STDOUT` | `1` | Standard output file descriptor. |
| `FD_FIRST_USABLE` | `2` | First file descriptor available for VFS files. |
| `FD_LIMIT` | `1024` | Maximum file descriptors per task. |
| `TIMER_HZ` | `100` | Timer interrupt frequency (100 Hz). |
| `NS_PER_TICK` | `10_000_000` | Nanoseconds per timer tick. |
| `PORT_MIN` | `0x0060` | Minimum I/O port for user-space access. |
| `DEFAULT_PIPE_CAPACITY` | `4096` | Default pipe buffer size. |
| `MAX_PENDING_HANDLES` | `64` | Maximum pending handle transfers per channel. |
| `TASK_ENTRY_SIZE` | `40` | Bytes per task entry in `list_tasks` output. |
| `EPIPE` | `-32` | Write to pipe with no reader. |
| `SIG_DFL` | `0` | Default signal handler. |
| `SIG_IGN` | `1` | Ignore signal handler. |
| `SIG_BLOCK` | `0` | sigprocmask: add to blocked mask. |
| `SIG_UNBLOCK` | `1` | sigprocmask: remove from blocked mask. |
| `SIG_SETMASK` | `2` | sigprocmask: set blocked mask. |
| `SEEK_SET` | `0` | Seek from beginning of file. |
| `SEEK_CUR` | `1` | Seek from current position. |
| `SEEK_END` | `2` | Seek from end of file. |
| `F_OK` | `0` | Test for existence. |
| `R_OK` | `1` | Test for read permission. |
| `W_OK` | `2` | Test for write permission. |
| `X_OK` | `4` | Test for execute permission. |

### D. SDK Examples

#### Hello World

```rust
use openos_sdk::console;

fn main() {
    console::writeln("Hello from OpenOS user-space!").unwrap();
}
```

#### IPC Channel

```rust
use openos_sdk::{channel, process};

fn main() {
    let (client, server) = channel::create().unwrap();

    // Send a message from the client end.
    channel::send(client, b"ping").unwrap();

    // Receive on the server end.
    let mut buf = [0u8; 64];
    let n = channel::receive(server, &mut buf).unwrap();

    process::exit(0);
}
```

#### RPC (Call/Reply)

```rust
use openos_sdk::{channel, process};

fn server(handle: openos_sdk::Handle) {
    let mut req = [0u8; 256];
    let n = channel::receive(handle, &mut req).unwrap();
    // process request...
    channel::reply(handle, b"OK").unwrap();
}

fn client(handle: openos_sdk::Handle) {
    let mut reply = [0u8; 256];
    let n = channel::call(handle, b"get_time", &mut reply).unwrap();
}
```

#### File I/O

```rust
use openos_sdk::fs;

fn main() {
    // Create and write a file.
    let fd = fs::create("hello.txt").unwrap();
    fs::write(fd, b"Hello, filesystem!").unwrap();
    fs::close(fd).unwrap();

    // Read it back.
    let fd = fs::open("hello.txt").unwrap();
    let mut buf = [0u8; 64];
    let n = fs::read(fd, &mut buf).unwrap();
    fs::close(fd).unwrap();
}
```

#### TCP Client

```rust
use openos_sdk::{socket, dns};

fn main() {
    let sock = socket::create_tcp().unwrap();

    let ip = dns::resolve("example.com").unwrap();
    let ip_u32 = u32::from_be_bytes(ip);

    socket::connect(sock, ip_u32, 80).unwrap();
    socket::send(sock, b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n").unwrap();

    let mut buf = [0u8; 4096];
    let n = socket::recv(sock, &mut buf).unwrap();

    socket::close(sock).unwrap();
}
```

#### Signals

```rust
use openos_sdk::signal;

extern "C" fn handler(sig: u32) {
    // handle SIGINT
}

fn main() {
    signal::signal(signal::SIGINT, handler as u64).unwrap();
    // ... application logic ...
}
```
