# OpenOS System Interface — Design Document

> A capability-based, channel-centric system interface for a microkernel OS.
> Deliberately non-POSIX. Designed for security, simplicity, and composability.

---

## 1. Philosophy

### What's Wrong with POSIX

POSIX is a 1988 standard that codified Unix assumptions no longer valid:

| POSIX Assumption | Reality |
|------------------|---------|
| File descriptors are ambient authority | Any code in a process can read/write any FD — no isolation |
| `fork()` is cheap | It copies the entire address space — CoW is a hack, not a solution |
| Signals are useful | Async, racy, lossy, poorly typed — universally hated |
| `ioctl()` is acceptable | Untyped catch-all — the "junk drawer" of system calls |
| `open()` works on everything | Files, sockets, devices, pipes — all crammed into one abstraction |
| Global file descriptor table | FDs are process-global state — impossible to delegate safely |
| `errno` is a good error model | A global variable, not a return value — thread-safety is a afterthought |

### What OpenOS Does Instead

| Principle | Implementation |
|-----------|---------------|
| **No ambient authority** | Every access requires a Handle with explicit Rights |
| **No fork()** | Processes are created explicitly: create → map memory → start thread |
| **No signals** | Async events use typed Notifications, delivered through Channels |
| **No ioctl** | Device-specific operations are messages to device servers |
| **No global state** | No global FD table, no errno — all state is in Handles |
| **Channel-centric IPC** | Channels are the primary primitive — everything else is built on top |

### Design Influences

- **seL4**: Minimal syscall count, capabilities as the only access control
- **Zircon (Fuchsia)**: Handles with rights, Channels with handle transfer, VMO/VMAR
- **QNX**: Synchronous message passing, pulses for async
- **Plan 9**: Per-process namespaces, resource managers as user-space servers

---

## 2. Core Concepts

### 2.1 Handles

A **Handle** is an opaque, unforgeable token that references a kernel object.
Handles are the *only* way user-space interacts with the kernel.

```
Handle = { object_id: u32, rights: Rights, generation: u32 }
```

- `object_id`: indexes into the kernel's object table
- `rights`: bitmask of allowed operations (see §2.2)
- `generation`: prevents use-after-close (A-B-A problem)

Handles are **process-local** — the same object may have different handles in
different processes with different rights. A handle in Process A has no meaning
in Process B unless explicitly transferred.

### 2.2 Rights

Rights are a bitmask attached to each handle. They can only be **narrowed**,
never amplified:

```
bit 0:  READ      — read data from the object
bit 1:  WRITE     — write data to the object
bit 2:  EXECUTE   — execute (for memory objects)
bit 3:  TRANSFER  — send this handle to another process via Channel
bit 4:  DUPLICATE — clone this handle within the same process
bit 5:  SIGNAL    — signal the object (e.g., trigger an Event)
bit 6:  WAIT      — wait on the object (e.g., Channel receive)
bit 7:  DESTROY   — close/destroy the object
bit 8:  MAP       — map into address space (for memory objects)
bit 9:  CONFIGURE — modify object properties
```

When a handle is transferred through a Channel, the sender specifies which
rights to grant. The resulting handle can only have *fewer* rights than the
original. This is **monotonic privilege reduction** — authority can only
decrease, never increase.

Convenience constants:
```rust
const RIGHTS_BASIC: Rights   = TRANSFER | DUPLICATE | WAIT | DESTROY;
const RIGHTS_IO: Rights      = READ | WRITE;
const RIGHTS_ALL: Rights     = 0x3FF;
```

### 2.3 Kernel Objects

The kernel manages a small set of typed objects:

| Object | Purpose | Key Operations |
|--------|---------|----------------|
| **Process** | Address space container | create, start, kill |
| **Thread** | Execution context | create, start, suspend, resume |
| **Channel** | Bidirectional message pipe | create, send, receive, call |
| **Event** | One-shot or level-triggered signal | create, signal, wait |
| **Memory** | Page-granularity memory region | create, map, unmap, resize |
| **Timer** | Deadline/interval timer | create, set, cancel |
| **Endpoint** | Named service endpoint | create, register, discover |
| **Job** | Process group with resource limits | create, attach, kill |

Every object is reference-counted. When the last handle to an object is closed,
the object is destroyed and its resources are reclaimed.

### 2.4 Channels

A **Channel** is a bidirectional, synchronous message pipe between two endpoints.
Channels are the *primary IPC mechanism* — everything else (RPC, service
discovery, event delivery) is built on channels.

Key properties:
- **Bidirectional**: both ends can send and receive
- **Synchronous**: `send` blocks until the receiver calls `receive`
- **Handle transfer**: messages can contain Handles, which are *moved* (not copied)
  from sender to receiver
- **No buffering**: at most one message in flight per direction

A Channel has two ends (handles). Typically:
- The **server** holds one end and calls `receive` in a loop
- The **client** holds the other end and calls `call` (send + block for reply)

```
  Client                          Server
    │                                │
    │  channel_call(msg)             │
    │  ─────────────────────────→    │  channel_receive(&msg)
    │                                │  process request
    │                                │  channel_reply(msg)
    │  ←─────────────────────────    │
    │  (unblocked with reply)        │
```

---

## 3. System Calls

### 3.1 Calling Convention

System calls use the `syscall` instruction:

```
RAX = syscall number
RDI = arg0 (Handle or primary argument)
RSI = arg1
RDX = arg2
R10 = arg3
R8  = arg4
R9  = arg5

Return:
  RAX = result (positive) or error code (negative)
```

Error codes are **negative return values**, not a global errno:

```
  0    = success
 -1    = EINVAL (invalid argument)
 -2    = ENOENT (not found)
 -3    = EPERM  (permission denied)
 -4    = ENOMEM (out of memory)
 -5    = EBUSY  (resource busy)
 -6    = EPIPE  (channel closed)
 -7    = EAGAIN (would block, non-blocking mode)
 -8    = ETIME  (timeout expired)
 -9    = EFAULT (bad pointer)
-10    = ENOSYS (unknown syscall)
```

### 3.2 Complete Syscall Table

**Total: 21 syscalls.** That's it.

```
 ┌─────────────────────────────────────────────────────────────────┐
 │  CHANNEL (4 syscalls)                                           │
 ├─────────────────────────────────────────────────────────────────┤
 │  channel_create   → (handle_a, handle_b)                        │
 │  channel_send     → handle, msg_ptr, msg_len, handles, count    │
 │  channel_receive  → handle, buf_ptr, buf_len, handles, count    │
 │  channel_call     → handle, msg_ptr, msg_len, reply_buf, len    │
 ├─────────────────────────────────────────────────────────────────┤
 │  HANDLE (3 syscalls)                                            │
 ├─────────────────────────────────────────────────────────────────┤
 │  handle_close     → handle                                      │
 │  handle_duplicate → handle, rights → new_handle                 │
 │  handle_transfer  → handle, target_channel, rights              │
 ├─────────────────────────────────────────────────────────────────┤
 │  MEMORY (3 syscalls)                                            │
 ├─────────────────────────────────────────────────────────────────┤
 │  memory_create    → size, flags → handle                        │
 │  memory_map       → handle, vaddr, size, flags → vaddr          │
 │  memory_unmap     → vaddr, size                                 │
 ├─────────────────────────────────────────────────────────────────┤
 │  PROCESS (4 syscalls)                                           │
 ├─────────────────────────────────────────────────────────────────┤
 │  process_create   → name, name_len → (proc_handle, vmar_handle)│
 │  process_start    → proc, thread, entry, stack, arg             │
 │  process_exit     → status (noreturn)                           │
 │  process_wait     → proc_handle → status                        │
 ├─────────────────────────────────────────────────────────────────┤
 │  THREAD (3 syscalls)                                            │
 ├─────────────────────────────────────────────────────────────────┤
 │  thread_create    → proc_handle → thread_handle                 │
 │  thread_exit      → (noreturn)                                  │
 │  thread_yield     →                                             │
 ├─────────────────────────────────────────────────────────────────┤
 │  EVENT (2 syscalls)                                             │
 ├─────────────────────────────────────────────────────────────────┤
 │  event_create     → flags → handle                              │
 │  event_signal     → handle                                      │
 ├─────────────────────────────────────────────────────────────────┤
 │  TIMER (1 syscall)                                              │
 ├─────────────────────────────────────────────────────────────────┤
 │  timer_create     → deadline_ns, interval_ns → handle           │
 ├─────────────────────────────────────────────────────────────────┤
 │  ENDPOINT (1 syscall)                                           │
 ├─────────────────────────────────────────────────────────────────┤
 │  endpoint_register → name, name_len, channel_handle             │
 └─────────────────────────────────────────────────────────────────┘
```

### 3.3 Syscall Details

#### channel_create

```
fn channel_create(flags: u64) → (handle_a: Handle, handle_b: Handle)
```

Creates a new Channel and returns handles to both ends.
`flags` is reserved (must be 0).

Both handles have `RIGHTS_BASIC | READ | WRITE | WAIT`.

#### channel_send

```
fn channel_send(
    handle: Handle,
    msg: *const u8,
    msg_len: u64,
    handles: *const Handle,
    handle_count: u64,
) → result: i64
```

Send a message through the channel. The message bytes are copied into the
kernel. Any Handles included in the message are **moved** — the sender's copies
are invalidated.

Blocks until the receiver calls `channel_receive`. Returns 0 on success.

#### channel_receive

```
fn channel_receive(
    handle: Handle,
    buf: *mut u8,
    buf_len: u64,
    handles: *mut Handle,
    handle_count: u64,
) → (bytes_read: u64, handles_received: u64)
```

Receive a message from the channel. Blocks until a message is available.
Received Handles are new handles in the caller's handle table.

#### channel_call

```
fn channel_call(
    handle: Handle,
    msg: *const u8,
    msg_len: u64,
    reply_buf: *mut u8,
    reply_len: u64,
) → reply_bytes: u64
```

Atomic send + block for reply. This is the primary RPC primitive.
The server receives the message, processes it, and calls `channel_reply`.
The client is unblocked with the reply data.

This is equivalent to `channel_send` + `channel_receive` but atomic —
the kernel guarantees no interleaving.

#### handle_close

```
fn handle_close(handle: Handle) → result: i64
```

Close a handle. If this is the last handle to the underlying object,
the object is destroyed and its resources are reclaimed.

#### handle_duplicate

```
fn handle_duplicate(handle: Handle, new_rights: Rights) → new_handle: Handle
```

Clone a handle within the same process, optionally narrowing rights.
`new_rights` must be a subset of the original handle's rights.

#### handle_transfer

```
fn handle_transfer(
    handle: Handle,
    target_channel: Handle,
    rights: Rights,
) → result: i64
```

Send a handle to another process via a Channel. The handle is **moved** —
it is no longer valid in the sender's handle table. The receiver gets a
new handle with the specified rights (which must be a subset of the original).

This is the **capability delegation** mechanism. To grant a process access to
a resource, send it a Handle through a Channel.

#### memory_create

```
fn memory_create(size: u64, flags: u64) → handle: Handle
```

Create a new memory object of the given size (rounded up to page boundary).
The memory is not mapped into any address space — use `memory_map` to access it.

Flags:
- `MEMORY_RESIZABLE` (bit 0): can be resized after creation
- `MEMORY_SHARED` (bit 1): can be mapped in multiple processes

#### memory_map

```
fn memory_map(
    handle: Handle,
    vaddr: u64,      // 0 = let kernel choose
    size: u64,
    offset: u64,
    flags: u64,
) → mapped_vaddr: u64
```

Map a memory object into the calling process's address space.

Flags:
- `MAP_READ` (bit 0): readable
- `MAP_WRITE` (bit 1): writable
- `MAP_EXEC` (bit 2): executable
- `MAP_FIXED` (bit 3): must map at exact vaddr

#### memory_unmap

```
fn memory_unmap(vaddr: u64, size: u64) → result: i64
```

Unmap a memory region. The underlying Memory object is not affected.

#### process_create

```
fn process_create(
    name: *const u8,
    name_len: u64,
) → (proc_handle: Handle, vmar_handle: Handle)
```

Create a new, empty process. Returns handles to the process and its
root virtual memory address region (VMAR). The process has no threads
and no memory mapped — use `memory_create` + `memory_map` to set it up,
then `process_start` to begin execution.

#### process_start

```
fn process_start(
    proc: Handle,
    thread: Handle,
    entry: u64,
    stack: u64,
    arg: u64,
) → result: i64
```

Start a thread in a process at the given entry point.
`stack` is the initial stack pointer.
`arg` is passed as the first argument to the entry function.

#### process_exit

```
fn process_exit(status: u64) → !
```

Terminate the calling process. All handles are closed, all memory is unmapped,
all threads are terminated. `status` is available to the parent via
`process_wait`.

#### process_wait

```
fn process_wait(proc: Handle, timeout_ns: u64) → (status: i64)
```

Wait for a process to exit. Returns the exit status.
If `timeout_ns` is 0, returns immediately with `EAGAIN` if still running.
If `timeout_ns` is `u64::MAX`, blocks indefinitely.

#### thread_create

```
fn thread_create(proc: Handle) → thread_handle: Handle
```

Create a new thread in the given process. The thread is suspended —
use `process_start` to begin execution.

#### thread_exit

```
fn thread_exit() → !
```

Terminate the calling thread. If it's the last thread in the process,
the process exits with status 0.

#### thread_yield

```
fn thread_yield() → result: i64
```

Voluntarily yield the CPU to the next ready thread.

#### event_create

```
fn event_create(flags: u64) → handle: Handle
```

Create a new event object.

Flags:
- `EVENT_ONESHOT` (bit 0): signal clears automatically after first wait
- `EVENT_LEVEL` (bit 1): signal persists until explicitly cleared

#### event_signal

```
fn event_signal(handle: Handle) → result: i64
```

Signal an event. All threads waiting on this event are unblocked.

#### timer_create

```
fn timer_create(deadline_ns: u64, interval_ns: u64) → handle: Handle
```

Create a timer. When `deadline_ns` is reached, the timer signals.
If `interval_ns` > 0, it repeats at that interval.

Timers are Event-compatible — they can be waited on via `channel_receive`
on a Channel that has the timer's event end.

#### endpoint_register

```
fn endpoint_register(
    name: *const u8,
    name_len: u64,
    channel: Handle,
) → result: i64
```

Register a named service endpoint. Other processes can discover the service
by name and receive a handle to the channel's server end.

This is the **service discovery** mechanism. A server registers its name;
a client discovers it and gets a Channel handle to communicate through.

---

## 4. Patterns

### 4.1 Basic RPC (Client-Server)

```
// Server
let (server, client) = channel_create(0);
endpoint_register("filesystem", 10, server);

loop {
    let (msg, handles) = channel_receive(server);
    let response = handle_request(msg);
    channel_reply(server, response);
}

// Client
let server = endpoint_discover("filesystem", 10);
let reply = channel_call(server, request_msg);
```

### 4.2 Capability Delegation

```
// Process A has a Memory handle with READ | WRITE rights.
// It wants to give Process B read-only access.

let (a_end, b_end) = channel_create(0);
// ... give b_end to Process B ...

// A narrows the handle and transfers it
handle_transfer(memory_handle, a_end, READ);
// memory_handle is now invalid in A

// B receives the handle with READ rights
let (msg, handles) = channel_receive(b_end);
let shared_mem = handles[0]; // Memory handle with READ only
memory_map(shared_mem, 0, size, 0, MAP_READ);
```

### 4.3 Process Creation (No fork)

```
// Parent creates a child process
let (proc, vmar) = process_create("child", 5);

// Load ELF into child's address space
let code_mem = memory_create(elf_size, 0);
// ... write ELF data into code_mem ...
memory_map(code_mem, 0, elf_size, 0, MAP_READ | MAP_EXEC);

let stack_mem = memory_create(stack_size, 0);
memory_map(stack_mem, 0, stack_size, 0, MAP_READ | MAP_WRITE);

// Create thread and start
let thread = thread_create(proc);
process_start(proc, thread, entry_point, stack_top, 0);

// Wait for child to exit
let status = process_wait(proc, u64::MAX);
```

### 4.4 Async Event via Channel

```
// A timer event is received through a Channel, not through signals.
let timer = timer_create(deadline, interval);
let (event_end, waiter_end) = channel_create(0);

// In a separate thread or via port multiplexing:
channel_receive(waiter_end); // blocks until timer fires
```

### 4.5 Service Discovery

```
// Server: register a named endpoint
let (server_ch, client_ch) = channel_create(0);
endpoint_register("display.server", 15, server_ch);

// Client: discover and connect
let server_ch = endpoint_discover("display.server", 15);
let reply = channel_call(server_ch, &OpenDisplay { width: 1920, height: 1080 });
```

---

## 5. What's NOT Here (and Why)

| Missing | Why |
|---------|-----|
| `fork()` | Explicit process creation is safer and more predictable |
| `exec()` | Loading ELF is done by the parent before `process_start` |
| `open()` / `read()` / `write()` | These are messages to service endpoints, not syscalls |
| `ioctl()` | Device-specific operations are typed messages, not magic numbers |
| `select()` / `poll()` | Channel multiplexing via Ports (like io_uring) |
| `signal()` / `kill()` | Events and Channels replace all signal use cases |
| `mmap()` / `munmap()` | `memory_create` + `memory_map` — explicit, typed |
| `pipe()` | Just `channel_create()` |
| `socket()` / `bind()` / `listen()` | Messages to a network service endpoint |
| `stat()` / `chmod()` | Messages to a filesystem service endpoint |
| `dup2()` | `handle_duplicate()` with explicit rights |
| `errno` | Return values are signed — negative means error |

---

## 6. Comparison with POSIX

| Operation | POSIX | OpenOS |
|-----------|-------|--------|
| Write to console | `write(1, buf, len)` | `channel_call(console_handle, msg)` |
| Read file | `read(fd, buf, len)` | `channel_call(fs_handle, ReadMsg { path, offset, len })` |
| Create process | `fork()` + `exec()` | `process_create()` + `memory_map()` + `process_start()` |
| Share memory | `shm_open()` + `mmap()` | `memory_create(SHARED)` + `handle_transfer()` |
| Network connect | `socket()` + `connect()` | `endpoint_discover("net.tcp")` + `channel_call()` |
| Wait for event | `select()` / `poll()` | `channel_receive()` on event channel |
| Timer | `timer_create()` + `signal()` | `timer_create()` → event → channel |
| Delegate access | Pass FD number (ambient) | `handle_transfer()` (explicit, rights-narrowed) |

---

## 7. Security Model

### 7.1 No Ambient Authority

A process cannot access *any* resource unless it holds a Handle with
appropriate Rights. There is no "current directory", no "environment variable",
no "default search path". Every resource access is explicit.

### 7.2 Capability Flow

Authority flows through Handle transfer via Channels:

```
  Boot Process (has all initial handles)
       │
       ├─→ handle_transfer(fs_handle, channel, READ)
       │       → Filesystem Server (read-only access to disk)
       │
       ├─→ handle_transfer(net_handle, channel, READ | WRITE)
       │       → Network Server (read-write access to NIC)
       │
       └─→ handle_transfer(console_handle, channel, WRITE)
               → User Shell (write-only access to console)
```

To revoke access: close the handle. The kernel automatically cleans up.

### 7.3 No TOCTOU Races

Since Channel communication is synchronous and atomic, there are no
time-of-check-to-time-of-use races. A server processes a request and
sends a reply in one transaction.

---

## 8. Implementation Roadmap

| Phase | Syscalls | Status |
|-------|----------|--------|
| **Phase 1** | `channel_create`, `channel_send`, `channel_receive`, `channel_call` | Current |
| **Phase 2** | `handle_close`, `handle_duplicate`, `handle_transfer` | Next |
| **Phase 3** | `memory_create`, `memory_map`, `memory_unmap` | Next |
| **Phase 4** | `process_create`, `process_start`, `process_exit`, `process_wait` | Future |
| **Phase 5** | `thread_create`, `thread_exit`, `thread_yield` | Future |
| **Phase 6** | `event_create`, `event_signal`, `timer_create` | Future |
| **Phase 7** | `endpoint_register` + service discovery | Future |

---

## Appendix A: Syscall Number Assignments

```rust
// Channel
pub const SYS_CHANNEL_CREATE:  u64 = 0x01;
pub const SYS_CHANNEL_SEND:    u64 = 0x02;
pub const SYS_CHANNEL_RECEIVE: u64 = 0x03;
pub const SYS_CHANNEL_CALL:    u64 = 0x04;

// Handle
pub const SYS_HANDLE_CLOSE:     u64 = 0x10;
pub const SYS_HANDLE_DUPLICATE: u64 = 0x11;
pub const SYS_HANDLE_TRANSFER:  u64 = 0x12;

// Memory
pub const SYS_MEMORY_CREATE: u64 = 0x20;
pub const SYS_MEMORY_MAP:    u64 = 0x21;
pub const SYS_MEMORY_UNMAP:  u64 = 0x22;

// Process
pub const SYS_PROCESS_CREATE: u64 = 0x30;
pub const SYS_PROCESS_START:  u64 = 0x31;
pub const SYS_PROCESS_EXIT:   u64 = 0x32;
pub const SYS_PROCESS_WAIT:   u64 = 0x33;

// Thread
pub const SYS_THREAD_CREATE: u64 = 0x40;
pub const SYS_THREAD_EXIT:   u64 = 0x41;
pub const SYS_THREAD_YIELD:  u64 = 0x42;

// Event / Timer
pub const SYS_EVENT_CREATE:  u64 = 0x50;
pub const SYS_EVENT_SIGNAL:  u64 = 0x51;
pub const SYS_TIMER_CREATE:  u64 = 0x52;

// Endpoint
pub const SYS_ENDPOINT_REGISTER: u64 = 0x60;
```

## Appendix B: Handle Representation

Handles are represented as 64-bit values in user-space:

```
bits [0:31]   object_id   — kernel object table index
bits [32:47]  rights      — capability rights bitmask
bits [48:63]  generation  — prevents use-after-close
```

The kernel validates the generation on every syscall. If the generation
doesn't match (the object was closed and reused), the syscall returns `EBADF`.

## Appendix C: Channel Message Format

Messages sent through Channels have a simple binary format:

```
Header (16 bytes):
  [0x00] msg_type: u16     — message type (application-defined)
  [0x02] flags:    u16     — reserved
  [0x04] tx_id:    u32     — transaction ID (for request-reply correlation)
  [0x08] data_len: u32     — payload length in bytes
  [0x0C] handle_count: u32 — number of handles following

Payload:
  [0x10..] data: [u8; data_len]

Handles:
  [after data] handles: [Handle; handle_count]
```

This is a *recommendation*, not a kernel-enforced format. The kernel treats
messages as opaque byte sequences. The header is a user-space convention.
