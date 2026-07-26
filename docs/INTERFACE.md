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

**Total: 22 syscalls.** That's it.

```
 ┌─────────────────────────────────────────────────────────────────┐
 │  CHANNEL (5 syscalls)                                           │
 ├─────────────────────────────────────────────────────────────────┤
 │  channel_create   → (handle_a, handle_b)                        │
 │  channel_send     → handle, msg_ptr, msg_len, handles, count    │
 │  channel_receive  → handle, buf_ptr, buf_len, handles, count    │
 │  channel_call     → handle, msg_ptr, msg_len, reply_buf, len    │
 │  channel_reply    → handle, msg_ptr, msg_len, handles, count    │
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
 │  process_create   → job, name, name_len → (proc, vmar)          │
 │  process_start    → proc, thread, entry, stack, arg             │
 │  process_exit     → status (noreturn)                           │
 │  process_wait     → proc_handle → status                        │
 ├─────────────────────────────────────────────────────────────────┤
 │  JOB (3 syscalls)                                               │
 ├─────────────────────────────────────────────────────────────────┤
 │  job_create       → parent_job, limits → job_handle             │
 │  job_attach       → job, proc_handle                            │
 │  job_kill         → job                                         │
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
 │  ENDPOINT (2 syscalls)                                          │
 ├─────────────────────────────────────────────────────────────────┤
 │  endpoint_register → name, name_len, channel_handle             │
 │  endpoint_discover → name, name_len → channel_handle            │
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
    reply_handles: *mut Handle,
    reply_handle_count: u64,
) → (reply_bytes: u64, handles_received: u64)
```

Atomic send + block for reply. This is the primary RPC primitive.
The server receives the message, processes it, and calls `channel_reply`.
The client is unblocked with the reply data.

This is equivalent to `channel_send` + `channel_receive` but atomic —
the kernel guarantees no interleaving.

**Server error handling**: The server MUST call `channel_reply` even on error.
The reply carries an application-level status code (first 8 bytes convention).
If the server crashes without replying, the kernel automatically sends an
`EPIPE` error to the blocked client — the client's `channel_call` returns
the negative error code. This prevents infinite client blocking.

**Server timeout**: The client can specify a timeout (via flags). If the server
does not reply within the deadline, `channel_call` returns `ETIME`.

#### channel_reply

```
fn channel_reply(
    handle: Handle,
    msg: *const u8,
    msg_len: u64,
    handles: *const Handle,
    handle_count: u64,
) → result: i64
```

Reply to a received message. Unblocks the caller's `channel_call`.
This is the server half of the RPC pattern.

**Must be called exactly once per `channel_receive`**. The kernel tracks
which received message is awaiting a reply. Calling `channel_reply` without
a pending receive returns `EINVAL`.

If the server process exits without replying, the kernel automatically
sends an `EPIPE` error reply to any blocked client.

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

**Lifecycle**: A Memory object is reference-counted. Each `memory_map` call
increments the refcount; each `memory_unmap` decrements it. When all handles
are closed AND all mappings are unmapped (refcount reaches 0), the physical
pages are freed. This means:

- If a process closes its Memory handle but the mapping remains, the pages
  stay alive — the mapping holds a reference.
- If a process unmaps but keeps the handle, the object stays alive.
- Only when both the handle is closed AND all mappings are removed does
  the memory get freed.

For `MEMORY_SHARED` objects mapped in multiple processes: each mapping in
each process counts as one reference. The object is freed only when ALL
mappings across ALL processes are removed AND all handles are closed.

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
The handle must have `MAP` right. The mapping holds an internal reference
to the Memory object (see lifecycle above).

Flags:
- `MAP_READ` (bit 0): readable
- `MAP_WRITE` (bit 1): writable
- `MAP_EXEC` (bit 2): executable
- `MAP_FIXED` (bit 3): must map at exact vaddr

#### memory_unmap

```
fn memory_unmap(vaddr: u64, size: u64) → result: i64
```

Unmap a memory region. Releases the internal reference on the underlying
Memory object. If this was the last reference and all handles are closed,
the physical pages are freed.

#### process_create

```
fn process_create(
    job: Handle,
    name: *const u8,
    name_len: u64,
) → (proc_handle: Handle, vmar_handle: Handle)
```

Create a new, empty process under the given Job. Returns handles to the
process and its root virtual memory address region (VMAR). The process
has no threads and no memory mapped — use `memory_create` + `memory_map`
to set it up, then `process_start` to begin execution.

The `job` handle determines the process's position in the process tree.
When the Job is killed, all processes within it are terminated.

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

**Orphan handling**: Every process belongs to exactly one **Job** (see below).
If a process exits, its child processes are NOT automatically killed — they
remain in the same Job. The Job itself manages the lifecycle. If a Job is
killed, all processes within it are terminated.

#### job_create

```
fn job_create(parent_job: Handle, limits: *const JobLimits) → job_handle: Handle
```

Create a new Job under a parent Job. A Job is a container for processes with
resource limits. The kernel creates a **root Job** at boot — all processes
ultimately belong to this tree.

`JobLimits` (passed by pointer):
```
struct JobLimits {
    max_memory:    u64,  // max total memory (bytes), 0 = inherit parent
    max_processes: u32,  // max process count, 0 = inherit parent
    max_threads:   u32,  // max thread count, 0 = inherit parent
}
```

#### job_attach

```
fn job_attach(job: Handle, proc: Handle) → result: i64
```

Attach a process to a Job. A process can only be in one Job at a time.
The process must not already be attached to a different Job.

#### job_kill

```
fn job_kill(job: Handle) → result: i64
```

Kill all processes in a Job and all child Jobs. This is the **bulk
termination** mechanism — used for cleaning up an entire service subtree.

**Orphan reclamation**: When a process exits, its children stay in the same
Job. If no one calls `process_wait` on the children, they remain until the
Job is killed. The root Job's owner (the init process) is responsible for
reaping orphaned processes. This is explicit — no implicit reparenting.

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

Register a named service endpoint in the **calling process's namespace**.
The namespace is per-process — each process has its own view of available
services, inspired by Plan 9's per-process namespaces.

A parent process can pre-populate a child's namespace by transferring
Channel handles before `process_start`. This gives the parent full control
over what services the child can access — no global service table, no
ambient service discovery.

**Naming**: Names are UTF-8 strings, hierarchical with `/` separators
(e.g., `fs/ext2`, `net/tcp`, `dev/serial0`). The namespace is a flat
map within each process — the `/` is a naming convention, not a filesystem.

**Conflicts**: Registering a name that already exists in the same namespace
returns `EBUSY`. Different processes can register the same name independently.

#### endpoint_discover

```
fn endpoint_discover(
    name: *const u8,
    name_len: u64,
) → channel_handle: Handle
```

Look up a named service in the **calling process's namespace**. Returns a
Channel handle to the server end, or `ENOENT` if not found.

This is the client half of service discovery. The server calls
`endpoint_register`; the client calls `endpoint_discover`.

**Inheritance**: When a process is created, its namespace is empty. The
parent must explicitly transfer service handles (via Channel handle transfer)
before the child starts. This is deliberate — no ambient authority, no
global service table. The parent decides what the child can see.

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
// Parent creates a Job for the child (inherits limits from parent's Job)
let child_job = job_create(parent_job, null);

// Parent creates a child process under that Job
let (proc, vmar) = process_create(child_job, "child", 5);

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

### 4.5 Service Discovery (Per-Process Namespace)

```
// Server: register a named endpoint in its own namespace
let (server_ch, client_ch) = channel_create(0);
endpoint_register("display.server", 15, server_ch);

// Parent: transfer the client end to the child's namespace
// (child's namespace starts empty — parent must populate it)
let (parent_end, child_end) = channel_create(0);
endpoint_register("display.server", 15, parent_end);
// ... transfer child_end to child via Channel handle transfer ...

// Client: discover from its own namespace
let server_ch = endpoint_discover("display.server", 15);
let reply = channel_call(server_ch, &OpenDisplay { width: 1920, height: 1080 });
```

The namespace is **per-process, not global**. A child process starts with an
empty namespace. The parent populates it by transferring Channel handles
before `process_start`. This means:

- No naming conflicts between unrelated processes
- The parent has full control over what services the child can access
- No ambient service discovery — explicit delegation only

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
// Channel (5)
pub const SYS_CHANNEL_CREATE:  u64 = 0x01;
pub const SYS_CHANNEL_SEND:    u64 = 0x02;
pub const SYS_CHANNEL_RECEIVE: u64 = 0x03;
pub const SYS_CHANNEL_CALL:    u64 = 0x04;
pub const SYS_CHANNEL_REPLY:   u64 = 0x05;

// Handle (3)
pub const SYS_HANDLE_CLOSE:     u64 = 0x10;
pub const SYS_HANDLE_DUPLICATE: u64 = 0x11;
pub const SYS_HANDLE_TRANSFER:  u64 = 0x12;

// Memory (3)
pub const SYS_MEMORY_CREATE: u64 = 0x20;
pub const SYS_MEMORY_MAP:    u64 = 0x21;
pub const SYS_MEMORY_UNMAP:  u64 = 0x22;

// Process (4)
pub const SYS_PROCESS_CREATE: u64 = 0x30;
pub const SYS_PROCESS_START:  u64 = 0x31;
pub const SYS_PROCESS_EXIT:   u64 = 0x32;
pub const SYS_PROCESS_WAIT:   u64 = 0x33;

// Thread (3)
pub const SYS_THREAD_CREATE: u64 = 0x40;
pub const SYS_THREAD_EXIT:   u64 = 0x41;
pub const SYS_THREAD_YIELD:  u64 = 0x42;

// Event / Timer (3)
pub const SYS_EVENT_CREATE:  u64 = 0x50;
pub const SYS_EVENT_SIGNAL:  u64 = 0x51;
pub const SYS_TIMER_CREATE:  u64 = 0x52;

// Job (3)
pub const SYS_JOB_CREATE:  u64 = 0x58;
pub const SYS_JOB_ATTACH:  u64 = 0x59;
pub const SYS_JOB_KILL:    u64 = 0x5a;

// Endpoint (2)
pub const SYS_ENDPOINT_REGISTER: u64 = 0x60;
pub const SYS_ENDPOINT_DISCOVER: u64 = 0x61;
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

## Appendix D: Rust SDK Error Handling

The kernel returns raw `i64` values (positive = success, negative = error).
The Rust SDK **immediately** converts these to `Result<T, Error>` — user-space
code never sees raw error codes.

```rust
// Kernel ABI layer (raw, unsafe)
mod abi {
    pub fn channel_call_raw(handle: u64, msg: *const u8, msg_len: u64,
                            reply: *mut u8, reply_len: u64) -> i64 {
        // inline assembly: syscall, return RAX
    }
}

// SDK layer (safe, typed)
pub mod sys {
    use crate::abi;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Error {
        InvalidArgument,   // -1
        NotFound,          // -2
        PermissionDenied,  // -3
        OutOfMemory,       // -4
        Busy,              // -5
        ChannelClosed,     // -6
        WouldBlock,        // -7
        Timeout,           // -8
        BadPointer,        // -9
        Unknown(i64),      // other
    }

    impl Error {
        /// Convert a raw kernel error code to a typed Error.
        /// Only called for negative return values.
        fn from_raw(code: i64) -> Self {
            match code {
                -1 => Self::InvalidArgument,
                -2 => Self::NotFound,
                -3 => Self::PermissionDenied,
                -4 => Self::OutOfMemory,
                -5 => Self::Busy,
                -6 => Self::ChannelClosed,
                -7 => Self::WouldBlock,
                -8 => Self::Timeout,
                -9 => Self::BadPointer,
                n  => Self::Unknown(n),
            }
        }
    }

    /// Convert a raw syscall result to Result<u64, Error>.
    fn result(raw: i64) -> Result<u64, Error> {
        if raw >= 0 {
            Ok(raw as u64)
        } else {
            Err(Error::from_raw(raw))
        }
    }

    // --- Typed syscall wrappers ---

    pub fn channel_call(
        handle: Handle,
        msg: &[u8],
        reply: &mut [u8],
    ) -> Result<(usize, usize), Error> {
        let raw = abi::channel_call_raw(
            handle.as_raw(),
            msg.as_ptr(), msg.len() as u64,
            reply.as_mut_ptr(), reply.len() as u64,
        );
        result(raw).map(|v| (v as usize, 0))
    }

    pub fn channel_create() -> Result<(Handle, Handle), Error> {
        let (a, b) = abi::channel_create_raw(0)?;
        Ok((Handle::from_raw(a), Handle::from_raw(b)))
    }

    pub fn process_create(name: &str) -> Result<(Handle, Handle), Error> {
        let (proc, vmar) = abi::process_create_raw(
            name.as_ptr(), name.len() as u64
        )?;
        Ok((Handle::from_raw(proc), Handle::from_raw(vmar)))
    }

    // ... etc for all 22 syscalls
}
```

**Design rules for the SDK**:

1. **No raw error codes leak** — every syscall wrapper returns `Result<T, Error>`
2. **Handles are typed** — `Handle` is a newtype, not a raw `u64`
3. **No `unsafe` in user-facing API** — all unsafe is in the `abi` module
4. **Builder pattern for complex operations** — `ProcessBuilder`, `ChannelBuilder`
5. **Zero-cost abstraction** — the `Result` conversion is a single comparison

```rust
// User code (never sees raw numbers)
use openos::sys;

let (server, client) = sys::channel_create()?;
let reply = sys::channel_call(server, &request, &mut reply_buf)?;
// reply is already a typed Result — no errno, no raw checks
```

## Appendix E: Glossary

Quick reference for core terms used throughout this document.

---

### Capability

An unforgeable token of authority that references a kernel object.
In OpenOS, capabilities are implemented as **Handles** with **Rights**.
A process cannot access any kernel resource unless it holds a capability
for it. Capabilities can be transferred (via Channel) but only narrowed,
never amplified. This eliminates ambient authority entirely.

*Compare*: POSIX file descriptors are ambient — any code in a process
can use any FD. OpenOS Handles are explicit — you must hold one.

---

### Channel

A bidirectional, synchronous message pipe between two endpoints.
Channels are the **primary IPC mechanism** in OpenOS. Both ends hold
a Handle. Messages are byte sequences that can contain other Handles
(capability transfer). At most one message is in flight per direction.

Key operations:
- `channel_create` — create a pair of connected Handles
- `channel_call` — atomic send + block for reply (client side)
- `channel_receive` + `channel_reply` — receive + reply (server side)

*Compare*: POSIX pipes are unidirectional byte streams with no handle
transfer. POSIX sockets require separate `socket`/`bind`/`listen`/`accept`.

---

### Endpoint

A named entry in a process's **namespace** that maps a service name
to a Channel's server Handle. Other processes in the same namespace
can call `endpoint_discover` to get a client Handle.

The namespace is **per-process** — each process has its own view of
available services, populated by the parent before `process_start`.

*Compare*: POSIX has a global filesystem namespace. OpenOS has per-process
service namespaces with no global table.

---

### Event

A signaling primitive. Can be one-shot (signal clears after first wait)
or level-triggered (signal persists until cleared). Used for async
notifications — the non-blocking counterpart to Channel's synchronous model.

*Compare*: POSIX signals are async, lossy, poorly typed, and universally
hated. OpenOS Events are typed, non-lossy, and delivered through Channels.

---

### Handle

An opaque, process-local token that references a kernel object.
The only way user-space interacts with the kernel. Each Handle carries
a **Rights** bitmask that restricts what operations are allowed.

Handles are 64-bit values encoding: object_id (32 bits), rights (16 bits),
generation (16 bits). The generation prevents use-after-close bugs.

Handles can be:
- **Closed** — releases the reference, may destroy the object
- **Duplicated** — cloned within the same process (rights can narrow)
- **Transferred** — moved to another process via Channel (rights can narrow)

*Compare*: POSIX file descriptors are small integers in a global table.
OpenOS Handles are opaque tokens with explicit rights and generation checks.

---

### Job

A container for processes with resource limits. Jobs form a **tree** —
every Job has a parent (except the root Job created at boot). Every
process belongs to exactly one Job.

`job_kill` terminates all processes in a Job and all child Jobs —
this is the bulk termination mechanism for cleaning up service subtrees.

There is no implicit reparenting: if a process exits, its children
stay in the same Job. The root Job's owner (init) is responsible for
reaping orphans.

*Compare*: POSIX has process groups and sessions, but orphan handling
is implicit (reparent to init). OpenOS Jobs make the hierarchy explicit.

---

### Memory Object

A kernel object representing a page-granularity region of physical memory.
Memory objects are **not** mapped into any address space by default —
`memory_map` is required to make them accessible.

Memory objects are reference-counted: each Handle and each mapping holds
a reference. Physical pages are freed only when the refcount reaches 0.

Flags:
- `MEMORY_RESIZABLE` — can grow/shrink after creation
- `MEMORY_SHARED` — can be mapped in multiple processes simultaneously

*Compare*: POSIX `mmap` conflates allocation and mapping. OpenOS separates
them: `memory_create` allocates, `memory_map` maps. This makes shared
memory explicit and transferable via Handle.

---

### Namespace

A per-process mapping from service names (UTF-8 strings) to Channel
Handles. Each process has its own namespace, populated by the parent
before `process_start`.

- `endpoint_register("name", channel)` — add entry to own namespace
- `endpoint_discover("name")` — look up entry in own namespace

A child starts with an **empty** namespace. The parent must explicitly
transfer service Handles to populate it. No ambient discovery.

*Compare*: POSIX has a global `/` namespace. Plan 9 has per-process
namespaces (mount/bind). OpenOS inherits Plan 9's approach but uses
Handle-based service discovery instead of file operations.

---

### Process

An isolated address space containing one or more Threads. Created
empty — the parent must map Memory objects and start a Thread explicitly.

Lifecycle:
1. `process_create(job, name)` → empty address space
2. `memory_create` + `memory_map` → populate address space
3. `thread_create` → create execution context
4. `process_start(proc, thread, entry, stack, arg)` → begin execution
5. `process_exit(status)` → terminate all threads, close all Handles
6. `process_wait(proc)` → parent reaps exit status

*Compare*: POSIX `fork()` copies everything, then `exec()` replaces it.
OpenOS creates an empty process and builds it up explicitly.

---

### Reference Count

The mechanism that governs kernel object lifetime. Each object tracks:
- Number of **Handles** pointing to it
- Number of **Mappings** using it (for Memory objects)

When all references are released, the object is destroyed and its
resources (physical pages, kernel memory) are reclaimed.

This prevents use-after-free and resource leaks without garbage collection.

---

### Reply

The server's response in an RPC exchange. After `channel_receive` delivers
a client's message, the server processes it and calls `channel_reply` to
send the response. This unblocks the client's `channel_call`.

If the server crashes without replying, the kernel automatically sends
an `EPIPE` error reply to the blocked client — preventing infinite waits.

*Compare*: POSIX has no built-in request-reply mechanism. Each operation
is independent. OpenOS makes the RPC pattern first-class.

---

### Rights

A bitmask attached to each Handle that restricts allowed operations.
Rights can only be **narrowed** (via `handle_duplicate` or `handle_transfer`),
never amplified.

| Right | Bit | Meaning |
|-------|-----|---------|
| `READ` | 0 | Read data from the object |
| `WRITE` | 1 | Write data to the object |
| `EXECUTE` | 2 | Execute (for Memory objects) |
| `TRANSFER` | 3 | Send Handle to another process |
| `DUPLICATE` | 4 | Clone Handle within same process |
| `SIGNAL` | 5 | Signal the object |
| `WAIT` | 6 | Wait on the object |
| `DESTROY` | 7 | Close/destroy the object |
| `MAP` | 8 | Map into address space |
| `CONFIGURE` | 9 | Modify object properties |

*Compare*: POSIX has `O_RDONLY`/`O_WRONLY`/`O_RDWR` on open. OpenOS has
10 independent right bits that can be combined arbitrarily per Handle.

---

### VMAR (Virtual Memory Address Region)

A named region within a process's virtual address space. Each process
has a root VMAR (returned by `process_create`). VMARs organize the
address space into logical regions — code, stack, heap, shared memory.

VMARs are used internally by `memory_map` to place Memory objects at
specific virtual addresses. The kernel tracks which VMAR owns which
address range to prevent overlaps.

*Compare*: POSIX `mmap` uses anonymous address ranges. OpenOS VMARs
are explicit objects that can be queried and managed.

---

### Thread

An execution context within a Process. Threads share the process's
address space and Handle table. Each thread has its own stack and
register state.

Created suspended — `process_start` is required to begin execution.
`thread_exit` terminates the current thread. If it's the last thread,
the process exits with status 0.

*Compare*: POSIX has `pthread_create` with complex attributes. OpenOS
threads are minimal — create, start, exit, yield.

---

### Timer

A deadline/interval signaling object. When the deadline is reached,
the timer signals (compatible with Event semantics). If an interval
is set, it repeats.

Timers are used for:
- Timeouts on `channel_call`
- Periodic tasks (heartbeat, polling)
- Sleep (`timer_create` + wait on the timer's event)

*Compare*: POSIX has `timer_create`, `timer_settime`, `nanosleep`,
`alarm`, `setitimer` — five separate mechanisms. OpenOS has one.

---

### VMO (Virtual Memory Object)

*Not used in OpenOS.* This is a Zircon (Fuchsia) concept — a resizable,
pageable memory container. OpenOS uses **Memory Objects** instead, which
are simpler (not pageable, fixed-size unless `MEMORY_RESIZABLE`).

Mentioned here for readers familiar with Zircon terminology.
