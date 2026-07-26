# OpenOS System Call Interface

This document defines the complete system call interface for OpenOS.
Both kernel (`kernel/src/syscall/`) and user-space (`sdk/src/lib.rs`) must
use the numbers and conventions defined here.

**Source of truth**: `kernel/src/syscall/number.rs` defines all syscall numbers.
This document is the human-readable reference.

---

## Table of Contents

1. [Calling Convention](#1-calling-convention)
2. [Error Codes](#2-error-codes)
3. [Handle System](#3-handle-system)
4. [Syscall Reference](#4-syscall-reference)
   - 4.1 [Channel (IPC)](#41-channel-ipc)
   - 4.2 [Handle Management](#42-handle-management)
   - 4.3 [Process](#43-process)
   - 4.4 [Memory](#44-memory)
   - 4.5 [Thread](#45-thread)
   - 4.6 [Signal](#46-signal)
   - 4.7 [Pipe](#47-pipe)
   - 4.8 [Filesystem](#48-filesystem)
   - 4.9 [Filesystem Metadata](#49-filesystem-metadata)
   - 4.10 [Symbolic Links](#410-symbolic-links)
   - 4.11 [Environment & Working Directory](#411-environment--working-directory)
   - 4.12 [File Descriptor Duplication](#412-file-descriptor-duplication)
   - 4.13 [Time & Sleep](#413-time--sleep)
   - 4.14 [Socket](#414-socket)
   - 4.15 [DNS](#415-dns)
   - 4.16 [Raw Network](#416-raw-network)
   - 4.17 [Hardware Access (User-Space Drivers)](#417-hardware-access-user-space-drivers)
   - 4.18 [Console (Debug)](#418-console-debug)
   - 4.19 [Event Signaling](#419-event-signaling)
   - 4.20 [Service Discovery](#420-service-discovery)
5. [SDK Examples](#5-sdk-examples)
6. [Appendix A: Syscall Number Table](#6-appendix-a-syscall-number-table)

---

## 1. Calling Convention

User-space invokes a syscall via the x86_64 `syscall` instruction.

**Register mapping** (arguments passed to the kernel):

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
- Negative = error code (see [Error Codes](#2-error-codes))

**Clobbered registers**: `rcx` and `r11` are destroyed by the `syscall`
instruction (Intel/AMD ABI).

**Maximum message size**: 4096 bytes. Pointers to user-space buffers are
validated to be non-null, below `USER_SPACE_MAX` (128 TiB), and within bounds.

---

## 2. Error Codes

Error codes are returned as negative values in `rax`.

| Code | Name | Description |
|------|------|-------------|
| -1   | `InvalidArgument` | An argument was invalid or out of range |
| -2   | `NotFound`        | The requested resource was not found |
| -3   | `PermissionDenied`| The caller lacks the required rights |
| -4   | `OutOfMemory`     | The kernel is out of memory |
| -5   | `Busy`            | The resource is busy (port in use, etc.) |
| -6   | `ChannelClosed`   | The channel has been closed by the peer |
| -7   | `WouldBlock`      | The operation would block (non-blocking mode) |
| -8   | `Timeout`         | The operation timed out |
| -9   | `BadPointer`      | The user-space pointer is invalid or in kernel space |
| -10  | `UnknownSyscall`  | The syscall number is not recognized |

---

## 3. Handle System

Handles are 64-bit opaque tokens that reference kernel objects (channels,
events, processes, etc.). They are the primary mechanism for user-space to
interact with kernel resources.

### Handle Bit Layout (64 bits)

```
Bits 0-27:   slot_id    (28 bits, max 268M handles)
Bits 28-37:  rights     (10 bits, permission bitmask)
Bits 38-63:  generation (26 bits, prevents use-after-close)
```

### Rights Bitmask

| Bit | Name | Description |
|-----|------|-------------|
| 0   | `READ`      | Read from the object |
| 1   | `WRITE`     | Write to the object |
| 2   | `EXECUTE`   | Execute (e.g., start a process) |
| 3   | `TRANSFER`  | Transfer the handle to another task |
| 4   | `DUPLICATE` | Duplicate the handle |
| 5   | `SIGNAL`    | Signal the object |
| 6   | `WAIT`      | Wait on the object |
| 7   | `DESTROY`   | Destroy the object |
| 8   | `MAP`       | Map the object into memory |
| 9   | `CONFIGURE` | Configure the object |

Rights can only be narrowed (monotonic reduction), never amplified.

---

## 4. Syscall Reference

### 4.1 Channel (IPC)

Channels are the fundamental IPC primitive. A channel has two endpoints
(A and B). Messages flow bidirectionally between endpoints.

---

#### `SYS_CHANNEL_CREATE` (0x01)

Create a new IPC channel and install both endpoints in the caller's handle table.

| Register | Value |
|----------|-------|
| `rax`    | `0x01` |

**Returns**: Handle value for End A (the "client" end) on success.
End B (the "server" end) is also installed in the handle table but its value
is not returned directly. Use `endpoint_register`/`endpoint_discover` or
`handle_transfer` to share End B.

**Errors**: `NotFound` (-2) if the handle table is full.

**SDK**:
```rust
let (handle_a, handle_b) = channel::create()?;
```

---

#### `SYS_CHANNEL_SEND` (0x02)

Send a message through a channel endpoint. If the peer is blocked in
`receive`, the message is delivered immediately and the peer is woken.
Otherwise, the message is stored.

| Register | Value |
|----------|-------|
| `rax`    | `0x02` |
| `rdi`    | Channel handle |
| `rsi`    | Pointer to message data |
| `rdx`    | Message length (max 4096) |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `ChannelClosed` (-6).

**SDK**:
```rust
channel::send(handle, b"hello")?;
```

---

#### `SYS_CHANNEL_RECEIVE` (0x03)

Receive a message from a channel endpoint. Blocks until a message is
available. When the system has only one runnable task, uses HLT spin-wait.

| Register | Value |
|----------|-------|
| `rax`    | `0x03` |
| `rdi`    | Channel handle |
| `rsi`    | Pointer to receive buffer |
| `rdx`    | Buffer length |

**Returns**: Number of bytes received on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `ChannelClosed` (-6).

**SDK**:
```rust
let mut buf = [0u8; 256];
let n = channel::receive(handle, &mut buf)?;
```

---

#### `SYS_CHANNEL_CALL` (0x04)

Atomic send-and-wait-for-reply (RPC primitive). Sends a message and blocks
until the peer calls `channel_reply`.

| Register | Value |
|----------|-------|
| `rax`    | `0x04` |
| `rdi`    | Channel handle |
| `rsi`    | Pointer to request message |
| `rdx`    | Request length |
| `r10`    | Pointer to reply buffer |
| `r8`     | Reply buffer length (reserved) |

**Returns**: Number of reply bytes on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `ChannelClosed` (-6).

**SDK**:
```rust
let mut reply = [0u8; 256];
let n = channel::call(handle, b"request", &mut reply)?;
```

---

#### `SYS_CHANNEL_REPLY` (0x05)

Reply to a pending `channel_call` from the peer. The reply data is stored
and the blocked caller is woken.

| Register | Value |
|----------|-------|
| `rax`    | `0x05` |
| `rdi`    | Channel handle |
| `rsi`    | Pointer to reply data |
| `rdx`    | Reply length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2).

**SDK**:
```rust
channel::reply(handle, b"response")?;
```

---

### 4.2 Handle Management

---

#### `SYS_HANDLE_CLOSE` (0x10)

Close a handle, removing it from the task's handle table. The underlying
kernel object is dropped if this was the last reference.

| Register | Value |
|----------|-------|
| `rax`    | `0x10` |
| `rdi`    | Handle value |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2) if the handle is invalid.

**SDK**:
```rust
handle::close(my_handle)?;
```

---

#### `SYS_HANDLE_DUPLICATE` (0x11)

Duplicate a handle with optionally narrowed rights. The new handle has a
fresh generation counter.

| Register | Value |
|----------|-------|
| `rax`    | `0x11` |
| `rdi`    | Handle to duplicate |
| `rsi`    | New rights mask (intersected with original) |

**Returns**: New handle value on success.

**Errors**: `NotFound` (-2), `PermissionDenied` (-3).

**SDK**:
```rust
let new_handle = handle::duplicate(handle, Rights::READ as u16)?;
```

---

#### `SYS_HANDLE_TRANSFER` (0x12)

Transfer a handle to another task via a channel. Removes the handle from
the sender's handle table and attaches it to the channel's pending handles.
The receiver gets the handle value as part of the next message.

| Register | Value |
|----------|-------|
| `rax`    | `0x12` |
| `rdi`    | Handle to transfer |
| `rsi`    | Channel handle to transfer through |
| `rdx`    | Rights for the transferred handle (reserved) |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2), `ChannelClosed` (-6).

**SDK**:
```rust
handle::transfer(my_handle, channel_handle)?;
```

---

### 4.3 Process

---

#### `SYS_PROCESS_CREATE` (0x30)

Create a new process (task) in the scheduler. The process is created but
not yet running. Call `process_start` to load an ELF and begin execution.

| Register | Value |
|----------|-------|
| `rax`    | `0x30` |
| `rdi`    | Job handle (unused, pass 0) |
| `rsi`    | Pointer to process name (UTF-8) |
| `rdx`    | Name length |

**Returns**: Task ID (positive) on success.

**Errors**: `BadPointer` (-9), `InvalidArgument` (-1), `OutOfMemory` (-4).

**SDK**:
```rust
let task_id = process::create("my_process")?;
```

---

#### `SYS_PROCESS_START` (0x31)

Start a process by loading an ELF binary from the disk filesystem or initrd.
Creates a new page table, loads ELF segments, and sets the task's context
to the ELF entry point.

| Register | Value |
|----------|-------|
| `rax`    | `0x31` |
| `rdi`    | Task ID (from `process_create`) |
| `rsi`    | Pointer to ELF filename (UTF-8) |
| `rdx`    | Filename length |

**Returns**: Task ID on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1),
`OutOfMemory` (-4).

**SDK**:
```rust
process::start(task_id, "hello.elf")?;
```

---

#### `SYS_PROCESS_EXIT` (0x32)

Exit the current process. Closes all handles, terminates the task, wakes
the parent if blocked in `process_wait`. Does not return.

| Register | Value |
|----------|-------|
| `rax`    | `0x32` |
| `rdi`    | Exit status code |

**Returns**: Never returns.

**SDK**:
```rust
process::exit(0); // does not return
```

---

#### `SYS_PROCESS_WAIT` (0x33)

Wait for a child process to exit.

| Register | Value |
|----------|-------|
| `rax`    | `0x33` |
| `rdi`    | Child task ID |
| `rsi`    | Timeout in ticks (0 = check once, `u64::MAX` = block forever) |

**Returns**: Child's exit status on success.

**Errors**: `WouldBlock` (-7) if timeout expires or non-blocking check finds
the child still running.

**SDK**:
```rust
let status = process::wait(child_id, u64::MAX)?;
```

---

#### `SYS_GETPID` (0x37)

Get the current process (task) ID.

| Register | Value |
|----------|-------|
| `rax`    | `0x37` |

**Returns**: Current task ID (always succeeds).

**SDK**:
```rust
let pid = process::getpid();
```

---

#### `SYS_GETPPID` (0x38)

Get the parent process ID.

| Register | Value |
|----------|-------|
| `rax`    | `0x38` |

**Returns**: Parent task ID, or 0 if the task has no parent.

**SDK**:
```rust
let ppid = process::getppid();
```

---

### 4.4 Memory

---

#### `SYS_BRK` (0x34)

Set the program break (heap end) address.

| Register | Value |
|----------|-------|
| `rax`    | `0x34` |
| `rdi`    | New break address (0 = query current) |

**Returns**: Current (or new) break address on success.

**Errors**: `InvalidArgument` (-1) if address is in kernel space.

**SDK**:
```rust
let current = memory::brk(0)?;           // query
let new_brk = memory::brk(0x4000_1000)?; // set
```

---

#### `SYS_MMAP` (0x35)

Map a region of virtual memory.

| Register | Value |
|----------|-------|
| `rax`    | `0x35` |
| `rdi`    | Hint address (0 = kernel chooses) |
| `rsi`    | Size in bytes |
| `rdx`    | Flags: bit 0 = writable, bit 1 = executable |

**Returns**: Virtual address of the mapped region.

**Errors**: `InvalidArgument` (-1), `OutOfMemory` (-4).

**SDK**:
```rust
use openos_sdk::memory::{MAP_READ, MAP_WRITE};
let addr = memory::mmap(0, 4096, MAP_READ | MAP_WRITE)?;
```

---

#### `SYS_MUNMAP` (0x36)

Unmap a region of virtual memory.

| Register | Value |
|----------|-------|
| `rax`    | `0x36` |
| `rdi`    | Virtual address (must be page-aligned) |
| `rsi`    | Size in bytes |

**Returns**: 0 on success.

**Errors**: `InvalidArgument` (-1).

**SDK**:
```rust
memory::munmap(addr, 4096)?;
```

---

### 4.5 Thread

---

#### `SYS_THREAD_CREATE` (0x40)

Create a new user-mode thread within the current process. The new thread
shares the parent's page table.

| Register | Value |
|----------|-------|
| `rax`    | `0x40` |
| `rdi`    | Entry point virtual address |
| `rsi`    | User stack pointer (top of stack) |
| `rdx`    | Argument passed to thread function (placed in RDI) |

**Returns**: New thread's task ID on success.

**Errors**: `InvalidArgument` (-1) if entry/stack is null or in kernel space,
`OutOfMemory` (-4).

**SDK**:
```rust
let tid = thread::create(entry_fn as usize, stack_top)?;
```

---

#### `SYS_THREAD_EXIT` (0x41)

Exit the current thread. Terminates the task with status 0.

| Register | Value |
|----------|-------|
| `rax`    | `0x41` |

**Returns**: Never returns.

**SDK**:
```rust
thread::exit(); // does not return
```

---

#### `SYS_THREAD_YIELD` (0x42)

Yield the CPU to the next ready task. The current task is moved to the
back of the ready queue.

| Register | Value |
|----------|-------|
| `rax`    | `0x42` |

**Returns**: 0 (always succeeds).

**SDK**:
```rust
thread::yield_();
```

---

### 4.6 Signal

Signals provide inter-process notification. Signal numbers 1..=31 are valid.
SIGKILL (9) cannot be caught or ignored.

| Signal | Number | Default Action | Description |
|--------|--------|----------------|-------------|
| `SIGINT`  | 2  | Terminate | Keyboard interrupt (Ctrl-C) |
| `SIGTRAP` | 5  | Stop | Trace/breakpoint trap |
| `SIGKILL` | 9  | Terminate | Kill (cannot be caught/ignored) |
| `SIGPIPE` | 13 | Terminate | Broken pipe |
| `SIGTERM` | 15 | Terminate | Termination request |
| `SIGCHLD` | 17 | Ignore | Child stopped or terminated |

Special handler addresses:
- `SIG_DFL` (0) -- kernel default action
- `SIG_IGN` (1) -- ignore the signal

---

#### `SYS_KILL` (0x44)

Send a signal to a task. If the target is blocked, it is woken so the
signal can be delivered on its next syscall entry.

| Register | Value |
|----------|-------|
| `rax`    | `0x44` |
| `rdi`    | Target task ID |
| `rsi`    | Signal number (1..=31) |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
signal::kill(target_pid, signal::SIGTERM)?;
```

---

#### `SYS_SIGNAL` (0x45)

Set the handler for a signal, returning the previous handler.

| Register | Value |
|----------|-------|
| `rax`    | `0x45` |
| `rdi`    | Signal number (1..=31) |
| `rsi`    | Handler address (`SIG_DFL`, `SIG_IGN`, or user address) |

**Returns**: Previous handler address.

**Errors**: `InvalidArgument` (-1), `PermissionDenied` (-3) for SIGKILL.

**SDK**:
```rust
let old = signal::signal(signal::SIGINT, my_handler as u64)?;
```

---

### 4.7 Pipe

---

#### `SYS_PIPE` (0x43)

Create a pipe pair. Installs read and write file descriptors in the
caller's fd table.

| Register | Value |
|----------|-------|
| `rax`    | `0x43` |
| `rdi`    | Pointer to `[u64; 2]` buffer for `[read_fd, write_fd]` |

**Returns**: 0 on success. The buffer receives `[read_fd, write_fd]`.

**Errors**: `BadPointer` (-9), `OutOfMemory` (-4).

---

### 4.8 Filesystem

File descriptors 0 and 1 are reserved for stdin and stdout. VFS file
descriptors start at 2 (up to 1024).

Writing to fd 1 (stdout) redirects to the serial console.

Paths are resolved against the task's current working directory. Absolute
paths (starting with `/`) are used as-is.

---

#### `SYS_FS_OPEN` (0xF7)

Open a file via VFS and return a file descriptor.

| Register | Value |
|----------|-------|
| `rax`    | `0xF7` |
| `rdi`    | Pointer to filename (UTF-8) |
| `rsi`    | Filename length |
| `rdx`    | Flags: 0 = read-only, 1 = write/create/truncate |

**Returns**: File descriptor (>= 2) on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `OutOfMemory` (-4).

**SDK**:
```rust
let fd = fs::open("data.txt")?;         // read-only
let fd = fs::create("output.txt")?;     // write/create/truncate
```

---

#### `SYS_FS_READ` (0xF8)

Read bytes from an open file descriptor. Also supports pipe reads.

| Register | Value |
|----------|-------|
| `rax`    | `0xF8` |
| `rdi`    | File descriptor |
| `rsi`    | Pointer to receive buffer |
| `rdx`    | Buffer length (max 4096) |

**Returns**: Number of bytes read (0 at EOF).

**Errors**: `NotFound` (-2), `BadPointer` (-9), `InvalidArgument` (-1).

**SDK**:
```rust
let mut buf = [0u8; 256];
let n = fs::read(fd, &mut buf)?;
```

---

#### `SYS_FS_WRITE` (0xF9)

Write bytes to an open file descriptor. Also supports pipe writes.
Writing to fd 1 (stdout) redirects to the serial console.

| Register | Value |
|----------|-------|
| `rax`    | `0xF9` |
| `rdi`    | File descriptor |
| `rsi`    | Pointer to data |
| `rdx`    | Data length |

**Returns**: Number of bytes written.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
let n = fs::write(fd, b"hello world")?;
```

---

#### `SYS_FS_SEEK` (0xFF)

Seek to a position in an open file descriptor.

| Register | Value |
|----------|-------|
| `rax`    | `0xFF` |
| `rdi`    | File descriptor |
| `rsi`    | Offset (interpreted as signed i64) |
| `rdx`    | Whence: 0 = SEEK_SET, 1 = SEEK_CUR, 2 = SEEK_END |

**Returns**: New absolute offset on success.

**Errors**: `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
let pos = fs::seek(fd, 0, 0)?; // seek to beginning
```

---

#### `SYS_FS_CLOSE` (0xFA)

Close a file descriptor.

| Register | Value |
|----------|-------|
| `rax`    | `0xFA` |
| `rdi`    | File descriptor |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
fs::close(fd)?;
```

---

### 4.9 Filesystem Metadata

---

#### `SYS_FS_UNLINK` (0xC0)

Delete a file by path.

| Register | Value |
|----------|-------|
| `rax`    | `0xC0` |
| `rdi`    | Pointer to path string |
| `rsi`    | Path length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
fs::unlink("old_file.txt")?;
```

---

#### `SYS_FS_RENAME` (0xC1)

Rename (move) a file from old path to new path. Implemented as copy + delete.

| Register | Value |
|----------|-------|
| `rax`    | `0xC1` |
| `rdi`    | Pointer to old path |
| `rsi`    | Old path length |
| `rdx`    | Pointer to new path |
| `r10`    | New path length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2).

**SDK**:
```rust
fs::rename("old.txt", "new.txt")?;
```

---

#### `SYS_FS_MKDIR` (0xC2)

Create a directory.

| Register | Value |
|----------|-------|
| `rax`    | `0xC2` |
| `rdi`    | Pointer to path string |
| `rsi`    | Path length |
| `rdx`    | Permissions (reserved, pass 0) |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `Busy` (-5) if already exists,
`InvalidArgument` (-1) if parent is not a directory.

**SDK**:
```rust
fs::mkdir("my_dir")?;
```

---

#### `SYS_FS_RMDIR` (0xC3)

Remove a directory.

| Register | Value |
|----------|-------|
| `rax`    | `0xC3` |
| `rdi`    | Pointer to path string |
| `rsi`    | Path length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
fs::rmdir("my_dir")?;
```

---

#### `SYS_FS_STAT` (0xC4)

Get file status/metadata.

| Register | Value |
|----------|-------|
| `rax`    | `0xC4` |
| `rdi`    | Pointer to path string |
| `rsi`    | Path length |
| `rdx`    | Pointer to 20-byte stat buffer |

**Stat buffer layout** (20 bytes, little-endian):

| Offset | Size | Field |
|--------|------|-------|
| 0      | 8    | `size` (u64) -- file size in bytes |
| 8      | 8    | `ino` (u64) -- inode number |
| 16     | 4    | `mode` (u32) -- `0o100_644` for files, `0o40_755` for directories |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2).

**SDK**:
```rust
let size = fs::file_size("data.txt")?;
```

---

#### `SYS_FS_READDIR` (0xC5)

Read directory entries. Returns null-terminated entry names packed into
the output buffer.

| Register | Value |
|----------|-------|
| `rax`    | `0xC5` |
| `rdi`    | Pointer to directory path |
| `rsi`    | Path length |
| `rdx`    | Pointer to output buffer (max 4096 bytes) |

**Returns**: Number of bytes written to buffer.

**Errors**: `BadPointer` (-9), `NotFound` (-2).

---

### 4.10 Symbolic Links

---

#### `SYS_SYMLINK` (0xCA)

Create a symbolic link.

| Register | Value |
|----------|-------|
| `rax`    | `0xCA` |
| `rdi`    | Pointer to target path |
| `rsi`    | Target path length |
| `rdx`    | Pointer to link path |
| `r10`    | Link path length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
fs::symlink("/disk/real_file", "/disk/link_name")?;
```

---

#### `SYS_READLINK` (0xCB)

Read the target of a symbolic link.

| Register | Value |
|----------|-------|
| `rax`    | `0xCB` |
| `rdi`    | Pointer to link path |
| `rsi`    | Link path length |
| `rdx`    | Pointer to output buffer |
| `r10`    | Output buffer length |

**Returns**: Number of bytes written to buffer.

**Errors**: `BadPointer` (-9), `NotFound` (-2).

**SDK**:
```rust
let target = fs::readlink("/disk/link_name")?;
```

---

### 4.11 Environment & Working Directory

Each task has its own key-value environment and working directory.

---

#### `SYS_ENV_GET` (0x48)

Get an environment variable by key.

| Register | Value |
|----------|-------|
| `rax`    | `0x48` |
| `rdi`    | Pointer to key string |
| `rsi`    | Key length |
| `rdx`    | Pointer to value output buffer |
| `r10`    | Value buffer length |

**Returns**: Length of the value on success, 0 if key does not exist.

**Errors**: `BadPointer` (-9), `InvalidArgument` (-1).

**SDK**:
```rust
if let Some(val) = env::get("PATH")? {
    // use val
}
```

---

#### `SYS_ENV_SET` (0x49)

Set an environment variable. Overwrites if the key already exists.

| Register | Value |
|----------|-------|
| `rax`    | `0x49` |
| `rdi`    | Pointer to key string |
| `rsi`    | Key length |
| `rdx`    | Pointer to value string |
| `r10`    | Value length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `InvalidArgument` (-1).

**SDK**:
```rust
env::set("HOME", "/home/user")?;
```

---

#### `SYS_CHDIR` (0xCD)

Change the current working directory. The path must exist and be a directory.

| Register | Value |
|----------|-------|
| `rax`    | `0xCD` |
| `rdi`    | Pointer to path string |
| `rsi`    | Path length |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
env::chdir("/disk/data")?;
```

---

#### `SYS_GETCWD` (0xCE)

Get the current working directory.

| Register | Value |
|----------|-------|
| `rax`    | `0xCE` |
| `rdi`    | Pointer to output buffer |
| `rsi`    | Buffer length |

**Returns**: Length of the cwd string.

**Errors**: `NotFound` (-2), `BadPointer` (-9), `InvalidArgument` (-1).

**SDK**:
```rust
let cwd = env::cwd()?;
```

---

### 4.12 File Descriptor Duplication

---

#### `SYS_DUP2` (0x47)

Duplicate a file descriptor. If `new_fd` is already open, it is closed first.

| Register | Value |
|----------|-------|
| `rax`    | `0x47` |
| `rdi`    | Old file descriptor |
| `rsi`    | New file descriptor |

**Returns**: `new_fd` on success.

**Errors**: `NotFound` (-2), `InvalidArgument` (-1) for reserved fds (0, 1) or
out-of-range fds (>= 1024).

**SDK**:
```rust
fs::dup2(old_fd, new_fd)?;
```

---

### 4.13 Time & Sleep

The kernel runs a timer at 100 Hz (10 ms per tick). Only `CLOCK_MONOTONIC`
(clock_id = 0) is supported.

---

#### `SYS_CLOCK_GETTIME` (0x3E)

Get the current time for a clock. Writes a 16-byte `timespec` to user-space.

| Register | Value |
|----------|-------|
| `rax`    | `0x3E` |
| `rdi`    | Clock identifier (0 = monotonic) |
| `rsi`    | Pointer to 16-byte buffer |

**Timespec layout** (16 bytes, little-endian):

| Offset | Size | Field |
|--------|------|-------|
| 0      | 8    | `sec` (u64) -- seconds since boot |
| 8      | 8    | `nsec` (u64) -- nanoseconds (0..999,999,999) |

**Returns**: 0 on success.

**Errors**: `InvalidArgument` (-1) if clock_id != 0, `BadPointer` (-9).

**SDK**:
```rust
let ts = time::clock_gettime(time::CLOCK_MONOTONIC)?;
```

---

#### `SYS_SLEEP` (0xF1)

Sleep for the specified number of timer ticks. Yields the CPU to other
tasks via context switch.

| Register | Value |
|----------|-------|
| `rax`    | `0xF1` |
| `rdi`    | Number of ticks to sleep |

**Returns**: 0 on success (always succeeds).

**SDK**:
```rust
time::sleep(10);       // sleep 10 ticks (~100 ms)
time::sleep_ms(500);   // sleep 500 ms
```

---

### 4.14 Socket

Socket descriptors are separate from file descriptors. Only TCP is
currently supported.

---

#### `SYS_SOCKET` (0xA0)

Create a socket.

| Register | Value |
|----------|-------|
| `rax`    | `0xA0` |
| `rdi`    | Socket type: 0 = TCP, 1 = UDP, 2 = Raw |

**Returns**: Socket descriptor on success.

**Errors**: `InvalidArgument` (-1), `NotFound` (-2).

**SDK**:
```rust
let sock = socket::create_tcp()?;
```

---

#### `SYS_BIND` (0xA1)

Bind a socket to a local port.

| Register | Value |
|----------|-------|
| `rax`    | `0xA1` |
| `rdi`    | Socket descriptor |
| `rsi`    | IPv4 address (network byte order, currently unused) |
| `rdx`    | Port number |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
socket::bind(sock, 8080)?;
```

---

#### `SYS_LISTEN` (0xA2)

Put a bound TCP socket into listening state. Registers the port in the
kernel's listen table for incoming SYN handling.

| Register | Value |
|----------|-------|
| `rax`    | `0xA2` |
| `rdi`    | Socket descriptor |

**Returns**: 0 on success.

**Errors**: `InvalidArgument` (-1) if not TCP or not bound, `NotFound` (-2).

**SDK**:
```rust
socket::listen(sock)?;
```

---

#### `SYS_ACCEPT` (0xA3)

Accept an incoming connection on a listening socket. Blocks until a
connection is available.

| Register | Value |
|----------|-------|
| `rax`    | `0xA3` |
| `rdi`    | Socket descriptor |

**Returns**: New socket descriptor for the accepted connection.

**Errors**: `InvalidArgument` (-1), `OutOfMemory` (-4).

**SDK**:
```rust
let client_sock = socket::accept(sock)?;
```

---

#### `SYS_CONNECT` (0xA4)

Connect a TCP socket to a remote address and port. Performs the TCP
three-way handshake.

| Register | Value |
|----------|-------|
| `rax`    | `0xA4` |
| `rdi`    | Socket descriptor |
| `rsi`    | Remote IPv4 address (network byte order, u32 in u64) |
| `rdx`    | Remote port |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2), `InvalidArgument` (-1), `Busy` (-5).

**SDK**:
```rust
socket::connect(sock, ip_u32, 80)?;
```

---

#### `SYS_SENDTO` (0xA5)

Send data on a connected TCP socket (or to a specific address for UDP).

| Register | Value |
|----------|-------|
| `rax`    | `0xA5` |
| `rdi`    | Socket descriptor |
| `rsi`    | Pointer to data buffer |
| `rdx`    | Data length |
| `r10`    | Destination IPv4 address (0 = use connected peer) |
| `r8`     | Destination port (0 = use connected peer) |

**Returns**: Number of bytes sent.

**Errors**: `BadPointer` (-9), `NotFound` (-2), `InvalidArgument` (-1).

**SDK**:
```rust
let n = socket::send(sock, b"GET / HTTP/1.1\r\n\r\n")?;
```

---

#### `SYS_RECVFROM` (0xA6)

Receive data from a socket (non-blocking).

| Register | Value |
|----------|-------|
| `rax`    | `0xA6` |
| `rdi`    | Socket descriptor |
| `rsi`    | Pointer to receive buffer |
| `rdx`    | Buffer length (max 4096) |

**Returns**: Number of bytes received.

**Errors**: `WouldBlock` (-7) if no data available, `NotFound` (-2),
`BadPointer` (-9).

**SDK**:
```rust
let mut buf = [0u8; 1024];
let n = socket::recv(sock, &mut buf)?;
```

---

#### `SYS_CLOSE_SOCK` (0xA7)

Close a socket. For connected TCP sockets, initiates connection teardown
(FIN). For listening sockets, unregisters the listen port.

| Register | Value |
|----------|-------|
| `rax`    | `0xA7` |
| `rdi`    | Socket descriptor |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2).

**SDK**:
```rust
socket::close(sock)?;
```

---

### 4.15 DNS

---

#### `SYS_DNS_RESOLVE` (0xA8)

Resolve a hostname to an IPv4 address using the kernel's DNS resolver.

| Register | Value |
|----------|-------|
| `rax`    | `0xA8` |
| `rdi`    | Pointer to hostname (UTF-8) |
| `rsi`    | Hostname length |
| `rdx`    | Pointer to 4-byte output buffer |

**Returns**: 0 on success. The 4-byte IPv4 address (network byte order)
is written to the output buffer.

**Errors**: `BadPointer` (-9), `NotFound` (-2) if resolution fails.

**SDK**:
```rust
let ip = dns::resolve("example.com")?; // [u8; 4]
```

---

### 4.16 Raw Network

Low-level Ethernet frame send/receive through the VirtIO-Net driver.
The TCP/IP stack runs in user-space on top of these primitives.

---

#### `SYS_NET_SEND` (0xFD)

Send a raw Ethernet frame.

| Register | Value |
|----------|-------|
| `rax`    | `0xFD` |
| `rdi`    | Pointer to frame data |
| `rsi`    | Frame length |

**Returns**: Number of bytes sent.

**Errors**: `BadPointer` (-9), `InvalidArgument` (-1).

**SDK**:
```rust
net::send_frame(&frame_data)?;
```

---

#### `SYS_NET_RECEIVE` (0xFE)

Receive a raw Ethernet frame (non-blocking).

| Register | Value |
|----------|-------|
| `rax`    | `0xFE` |
| `rdi`    | Pointer to receive buffer |
| `rsi`    | Buffer length |

**Returns**: Number of bytes received.

**Errors**: `WouldBlock` (-7) if no frame available, `BadPointer` (-9).

**SDK**:
```rust
let mut buf = [0u8; 1518];
let n = net::receive_frame(&mut buf)?;
```

---

### 4.17 Hardware Access (User-Space Drivers)

These syscalls enable user-space device drivers. Port I/O is restricted
to ports >= 0x0060 to prevent interference with legacy system devices
(PIC, PIT, VGA). Port 0x60 (keyboard data) is allowed.

---

#### `SYS_PORT_IN` (0xB0)

Read from an I/O port.

| Register | Value |
|----------|-------|
| `rax`    | `0xB0` |
| `rdi`    | Port address (u16) |
| `rsi`    | Size in bytes: 1, 2, or 4 |

**Returns**: Value read from the port.

**Errors**: `InvalidArgument` (-1) for invalid size, `PermissionDenied` (-3)
for ports < 0x0060.

**SDK**:
```rust
let val = device::port_in(0x60, 1)?; // read keyboard data byte
```

---

#### `SYS_PORT_OUT` (0xB1)

Write to an I/O port.

| Register | Value |
|----------|-------|
| `rax`    | `0xB1` |
| `rdi`    | Port address (u16) |
| `rsi`    | Value to write |
| `rdx`    | Size in bytes: 1, 2, or 4 |

**Returns**: 0 on success.

**Errors**: `InvalidArgument` (-1), `PermissionDenied` (-3) for ports < 0x0060.

**SDK**:
```rust
device::port_out(0x64, 0xAE, 1)?; // keyboard controller command
```

---

#### `SYS_MMIO_MAP` (0xB2)

Map a physical MMIO region into the current task's virtual address space.
Both address and size must be page-aligned (4 KiB). The region is mapped
uncached.

| Register | Value |
|----------|-------|
| `rax`    | `0xB2` |
| `rdi`    | Physical address (page-aligned) |
| `rsi`    | Size in bytes (page-aligned) |

**Returns**: Virtual address of the mapped region.

**Errors**: `InvalidArgument` (-1) for non-aligned values, `OutOfMemory` (-4).

**SDK**:
```rust
let virt = device::mmio_map(0xFED0_0000, 0x1000)?;
```

---

#### `SYS_MMIO_UNMAP` (0xB3)

Unmap a previously mapped MMIO region.

| Register | Value |
|----------|-------|
| `rax`    | `0xB3` |
| `rdi`    | Virtual address (page-aligned) |
| `rsi`    | Size in bytes (page-aligned) |

**Returns**: 0 on success.

**Errors**: `InvalidArgument` (-1).

**SDK**:
```rust
device::mmio_unmap(virt, 0x1000)?;
```

---

#### `SYS_IRQ_WAIT` (0xB4)

Wait for a hardware IRQ event to be signaled. The handle must reference
an `IrqEvent` kernel object (created by the kernel's IRQ forwarding system).

| Register | Value |
|----------|-------|
| `rax`    | `0xB4` |
| `rdi`    | Handle to `IrqEvent` |
| `rsi`    | Blocking flag: 0 = non-blocking, non-zero = blocking |

**Returns**: Device data byte captured by the IRQ handler (e.g., keyboard
scancode for IRQ 1).

**Errors**: `WouldBlock` (-7) if non-blocking and not signaled,
`NotFound` (-2).

**SDK**:
```rust
let scancode = device::irq_wait(kb_handle, true)?;
```

---

### 4.18 Console (Debug)

OpenOS-specific debug syscalls for serial port I/O and keyboard input.

---

#### `SYS_CONSOLE_WRITE` (0xF0)

Write bytes to the kernel debug console (serial port).

| Register | Value |
|----------|-------|
| `rax`    | `0xF0` |
| `rdi`    | Pointer to data |
| `rsi`    | Data length |

**Returns**: Number of bytes written.

**Errors**: `BadPointer` (-9).

**SDK**:
```rust
console::write("hello")?;
console::writeln("hello")?;
```

---

#### `SYS_CONSOLE_READ` (0xF4)

Read characters from the keyboard input buffer.

| Register | Value |
|----------|-------|
| `rax`    | `0xF4` |
| `rdi`    | Pointer to receive buffer |
| `rsi`    | Buffer length |
| `rdx`    | Flags: bit 0 = blocking |

**Returns**: Number of bytes read.

**Errors**: `InvalidArgument` (-1), `BadPointer` (-9).

**SDK**:
```rust
let mut buf = [0u8; 64];
let n = console::read(&mut buf, true)?;
```

---

### 4.19 Event Signaling

Events are kernel objects for inter-task synchronization. An event starts
unsignaled. `signal` transitions it to signaled. `wait` blocks until
signaled, then clears the signal (level-triggered).

---

#### `SYS_EVENT_CREATE` (0xF2)

Create a new unsignaled event. Returns a handle.

| Register | Value |
|----------|-------|
| `rax`    | `0xF2` |

**Returns**: Handle to the event.

**Errors**: `NotFound` (-2).

**SDK**:
```rust
let evt = event::create()?;
```

---

#### `SYS_EVENT_SIGNAL` (0xF3)

Signal an event. Wakes any task blocked in `event_wait`.

| Register | Value |
|----------|-------|
| `rax`    | `0xF3` |
| `rdi`    | Event handle |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2).

**SDK**:
```rust
event::signal(evt)?;
```

---

#### `SYS_EVENT_WAIT` (0xFB)

Block until an event is signaled, then clear the signal.

| Register | Value |
|----------|-------|
| `rax`    | `0xFB` |
| `rdi`    | Event handle |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2).

**SDK**:
```rust
event::wait(evt)?;
```

---

#### `SYS_EVENT_DESTROY` (0xFC)

Destroy an event by closing its handle.

| Register | Value |
|----------|-------|
| `rax`    | `0xFC` |
| `rdi`    | Event handle |

**Returns**: 0 on success.

**Errors**: `NotFound` (-2).

**SDK**:
```rust
event::destroy(evt)?;
```

---

### 4.20 Service Discovery

Service discovery allows tasks to register and find named service
endpoints in their namespace.

---

#### `SYS_ENDPOINT_REGISTER` (0xF5)

Register a named service endpoint in the current task's namespace.

| Register | Value |
|----------|-------|
| `rax`    | `0xF5` |
| `rdi`    | Pointer to service name (UTF-8) |
| `rsi`    | Name length |
| `rdx`    | Handle to the Channel server end |

**Returns**: 0 on success.

**Errors**: `BadPointer` (-9), `NotFound` (-2).

**SDK**:
```rust
service::register("console", server_handle)?;
```

---

#### `SYS_ENDPOINT_DISCOVER` (0xF6)

Discover a named service endpoint in the current task's namespace.

| Register | Value |
|----------|-------|
| `rax`    | `0xF6` |
| `rdi`    | Pointer to service name (UTF-8) |
| `rsi`    | Name length |

**Returns**: Handle to the Channel server end.

**Errors**: `BadPointer` (-9), `NotFound` (-2) if not registered.

**SDK**:
```rust
let handle = service::discover("console")?;
```

---

## 5. SDK Examples

### Hello World

```rust
use openos_sdk::console;

fn main() {
    console::writeln("Hello from OpenOS user-space!").unwrap();
}
```

### IPC Channel

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

### RPC (Call/Reply)

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

### File I/O

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

### TCP Client

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

### Memory Mapping

```rust
use openos_sdk::memory;

fn main() {
    let addr = memory::mmap(0, 4096, memory::MAP_READ | memory::MAP_WRITE).unwrap();

    // Use the mapped memory...

    memory::munmap(addr, 4096).unwrap();
}
```

### Signals

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

### Environment Variables

```rust
use openos_sdk::env;

fn main() {
    env::set("APP_NAME", "myapp").unwrap();

    if let Ok(Some(name)) = env::get("APP_NAME") {
        // use name
    }

    env::chdir("/disk/data").unwrap();
    let cwd = env::cwd().unwrap();
}
```

---

## 6. Appendix A: Syscall Number Table

### Channel (IPC) -- Range 0x01-0x0F

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0x01` | `SYS_CHANNEL_CREATE` | -- | handle |
| `0x02` | `SYS_CHANNEL_SEND` | handle, ptr, len | 0 |
| `0x03` | `SYS_CHANNEL_RECEIVE` | handle, buf_ptr, buf_len | bytes received |
| `0x04` | `SYS_CHANNEL_CALL` | handle, msg_ptr, msg_len, reply_ptr, reply_len | reply bytes |
| `0x05` | `SYS_CHANNEL_REPLY` | handle, ptr, len | 0 |

### Handle -- Range 0x10-0x1F

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0x10` | `SYS_HANDLE_CLOSE` | handle | 0 |
| `0x11` | `SYS_HANDLE_DUPLICATE` | handle, rights | new handle |
| `0x12` | `SYS_HANDLE_TRANSFER` | handle, channel, rights | 0 |

### Process -- Range 0x30-0x3F

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0x30` | `SYS_PROCESS_CREATE` | job, name_ptr, name_len | task id |
| `0x31` | `SYS_PROCESS_START` | task_id, elf_ptr, elf_len | task id |
| `0x32` | `SYS_PROCESS_EXIT` | status | (no return) |
| `0x33` | `SYS_PROCESS_WAIT` | child_id, timeout | exit status |
| `0x34` | `SYS_BRK` | new_brk | break address |
| `0x35` | `SYS_MMAP` | hint, size, flags | virtual address |
| `0x36` | `SYS_MUNMAP` | addr, size | 0 |
| `0x37` | `SYS_GETPID` | -- | task id |
| `0x38` | `SYS_GETPPID` | -- | parent task id |
| `0x3E` | `SYS_CLOCK_GETTIME` | clock_id, timespec_ptr | 0 |

### Thread -- Range 0x40-0x4F

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0x40` | `SYS_THREAD_CREATE` | entry, stack, arg | thread task id |
| `0x41` | `SYS_THREAD_EXIT` | -- | (no return) |
| `0x42` | `SYS_THREAD_YIELD` | -- | 0 |
| `0x43` | `SYS_PIPE` | fds_ptr | 0 |
| `0x44` | `SYS_KILL` | pid, sig | 0 |
| `0x45` | `SYS_SIGNAL` | sig, handler | old handler |
| `0x47` | `SYS_DUP2` | old_fd, new_fd | new_fd |
| `0x48` | `SYS_ENV_GET` | key_ptr, key_len, val_ptr, val_len | value length |
| `0x49` | `SYS_ENV_SET` | key_ptr, key_len, val_ptr, val_len | 0 |

### Socket / DNS -- Range 0xA0-0xAF

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0xA0` | `SYS_SOCKET` | type | socket fd |
| `0xA1` | `SYS_BIND` | sock_fd, addr, port | 0 |
| `0xA2` | `SYS_LISTEN` | sock_fd | 0 |
| `0xA3` | `SYS_ACCEPT` | sock_fd | new socket fd |
| `0xA4` | `SYS_CONNECT` | sock_fd, addr, port | 0 |
| `0xA5` | `SYS_SENDTO` | sock_fd, ptr, len, addr, port | bytes sent |
| `0xA6` | `SYS_RECVFROM` | sock_fd, buf_ptr, buf_len | bytes received |
| `0xA7` | `SYS_CLOSE_SOCK` | sock_fd | 0 |
| `0xA8` | `SYS_DNS_RESOLVE` | name_ptr, name_len, out_ptr | 0 |

### Hardware Access -- Range 0xB0-0xBF

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0xB0` | `SYS_PORT_IN` | port, size | value |
| `0xB1` | `SYS_PORT_OUT` | port, value, size | 0 |
| `0xB2` | `SYS_MMIO_MAP` | phys_addr, size | virtual address |
| `0xB3` | `SYS_MMIO_UNMAP` | virt_addr, size | 0 |
| `0xB4` | `SYS_IRQ_WAIT` | handle, blocking | device data |

### Filesystem Metadata -- Range 0xC0-0xCF

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0xC0` | `SYS_FS_UNLINK` | path_ptr, path_len | 0 |
| `0xC1` | `SYS_FS_RENAME` | old_ptr, old_len, new_ptr, new_len | 0 |
| `0xC2` | `SYS_FS_MKDIR` | path_ptr, path_len, perms | 0 |
| `0xC3` | `SYS_FS_RMDIR` | path_ptr, path_len | 0 |
| `0xC4` | `SYS_FS_STAT` | path_ptr, path_len, out_ptr | 0 |
| `0xC5` | `SYS_FS_READDIR` | path_ptr, path_len, out_ptr | bytes written |
| `0xCA` | `SYS_SYMLINK` | target_ptr, target_len, link_ptr, link_len | 0 |
| `0xCB` | `SYS_READLINK` | path_ptr, path_len, buf_ptr, buf_len | bytes written |
| `0xCD` | `SYS_CHDIR` | path_ptr, path_len | 0 |
| `0xCE` | `SYS_GETCWD` | buf_ptr, buf_len | cwd length |

### OpenOS-Specific -- Range 0xF0-0xFF

| Number | Name | Arguments | Returns |
|--------|------|-----------|---------|
| `0xF0` | `SYS_CONSOLE_WRITE` | ptr, len | bytes written |
| `0xF1` | `SYS_SLEEP` | ticks | 0 |
| `0xF2` | `SYS_EVENT_CREATE` | -- | handle |
| `0xF3` | `SYS_EVENT_SIGNAL` | handle | 0 |
| `0xF4` | `SYS_CONSOLE_READ` | buf_ptr, buf_len, flags | bytes read |
| `0xF5` | `SYS_ENDPOINT_REGISTER` | name_ptr, name_len, handle | 0 |
| `0xF6` | `SYS_ENDPOINT_DISCOVER` | name_ptr, name_len | handle |
| `0xF7` | `SYS_FS_OPEN` | name_ptr, name_len, flags | fd |
| `0xF8` | `SYS_FS_READ` | fd, buf_ptr, buf_len | bytes read |
| `0xF9` | `SYS_FS_WRITE` | fd, data_ptr, data_len | bytes written |
| `0xFA` | `SYS_FS_CLOSE` | fd | 0 |
| `0xFB` | `SYS_EVENT_WAIT` | handle | 0 |
| `0xFC` | `SYS_EVENT_DESTROY` | handle | 0 |
| `0xFD` | `SYS_NET_SEND` | ptr, len | bytes sent |
| `0xFE` | `SYS_NET_RECEIVE` | buf_ptr, buf_len | bytes received |
| `0xFF` | `SYS_FS_SEEK` | fd, offset, whence | new offset |

**Total**: 67 syscalls across 10 subsystems.
