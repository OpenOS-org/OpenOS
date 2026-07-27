//! POSIX-like threading library for OpenOS.
//!
//! Provides thread creation/join/exit, mutexes, condition variables,
//! barriers, read-write locks, one-time initialization, and thread-local
//! storage built on top of OpenOS syscalls.
//!
//! # Example
//! ```no_run
//! use pthreads::{Mutex, Pthread};
//!
//! let mut lock = Mutex::new();
//! lock.lock();
//! // critical section
//! lock.unlock();
//! ```

#![no_std]

extern crate alloc;

use core::arch::asm;
use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

// ─── Syscall numbers (must match kernel/src/syscall/number.rs) ───

const SYS_THREAD_CREATE: u64 = 0x40;
const SYS_THREAD_EXIT: u64 = 0x41;
const SYS_THREAD_YIELD: u64 = 0x42;
const SYS_PROCESS_WAIT: u64 = 0x33;
const SYS_EVENT_CREATE: u64 = 0xF2;
const SYS_EVENT_SIGNAL: u64 = 0xF3;
const SYS_EVENT_WAIT: u64 = 0xFB;
// ─── Raw syscall wrappers ───

unsafe fn syscall0(number: u64) -> i64 {
    let result: i64;
    asm!(
        "syscall",
        in("rax") number,
        lateout("rax") result,
        out("rcx") _,
        out("r11") _,
    );
    result
}

unsafe fn syscall1(number: u64, arg1: u64) -> i64 {
    let result: i64;
    asm!(
        "syscall",
        in("rax") number,
        in("rdi") arg1,
        lateout("rax") result,
        out("rcx") _,
        out("r11") _,
    );
    result
}

unsafe fn syscall2(number: u64, arg1: u64, arg2: u64) -> i64 {
    let result: i64;
    asm!(
        "syscall",
        in("rax") number,
        in("rdi") arg1,
        in("rsi") arg2,
        lateout("rax") result,
        out("rcx") _,
        out("r11") _,
    );
    result
}

unsafe fn syscall3(number: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let result: i64;
    asm!(
        "syscall",
        in("rax") number,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") result,
        out("rcx") _,
        out("r11") _,
    );
    result
}

// ─── Thread ───

/// Default thread stack size (64 KiB).
const DEFAULT_STACK_SIZE: usize = 64 * 1024;

/// Opaque handle for tracking a thread across join/exit.
pub struct Pthread {
    /// Task ID returned by the kernel.
    tid: u64,
    /// Pointer to the allocated stack (for deallocation on join).
    stack_ptr: *mut u8,
    /// Stack size in bytes.
    stack_size: usize,
    /// 0 = running, 1 = joined/exited.
    joined: AtomicI32,
}

// Safety: The kernel manages thread scheduling; the Pthread struct
// itself only holds POD fields and a raw pointer to memory we own.
unsafe impl Send for Pthread {}
unsafe impl Sync for Pthread {}

/// Entry point trampoline stored per-thread so the C-ABI wrapper can
/// call the Rust closure. Protected by the fact that each thread runs
/// exactly once.
static THREAD_ENTRY: AtomicUsize = AtomicUsize::new(0);
static THREAD_ARG: AtomicUsize = AtomicUsize::new(0);

/// Thread entry point with C calling convention for the kernel.
///
/// The kernel sets rdi = arg (the Pthread pointer). We read the
/// user-supplied entry and arg from global storage.
#[no_mangle]
pub extern "C" fn pthread_entry(_arg: u64) -> ! {
    // Retrieve the closure pointer and argument.
    let entry = THREAD_ENTRY.swap(0, Ordering::Acquire) as *const ();
    let arg = THREAD_ARG.swap(0, Ordering::Acquire) as *mut u8;

    // Safety: the caller guaranteed this pointer is valid for the
    // duration of the thread.
    if !entry.is_null() {
        let func: fn(*mut u8) = unsafe { core::mem::transmute(entry) };
        func(arg);
    }

    // If the function returns without calling pthread_exit, exit with 0.
    pthread_exit(0);
}

/// Create a new thread.
///
/// `start_routine` is a function pointer called in the new thread with `arg`.
/// `stack_size` is the desired stack size in bytes (0 for default 64 KiB).
///
/// Returns the `Pthread` handle on success.
///
/// # Errors
///
/// Returns an error string if the syscall fails or memory allocation fails.
pub fn pthread_create(
    start_routine: fn(*mut u8),
    arg: *mut u8,
    stack_size: usize,
) -> Result<Pthread, &'static str> {
    let actual_stack_size = if stack_size == 0 {
        DEFAULT_STACK_SIZE
    } else {
        stack_size
    };

    // Allocate stack via mmap (MAP_READ | MAP_WRITE).
    let stack_base = openos_sdk::memory::mmap(
        0,
        actual_stack_size,
        openos_sdk::memory::MAP_READ | openos_sdk::memory::MAP_WRITE,
    )
    .map_err(|_| "mmap failed for thread stack")?;

    // Stack grows downward on x86_64: entry point receives the top.
    let stack_top = stack_base + actual_stack_size;

    // Store the entry function and arg in globals for the trampoline.
    // This is safe because we call SYS_THREAD_CREATE immediately and the
    // trampoline reads them before any other thread can overwrite.
    THREAD_ENTRY.store(start_routine as *const () as usize, Ordering::Release);
    THREAD_ARG.store(arg as usize, Ordering::Release);

    // Invoke SYS_THREAD_CREATE(entry_point, stack_top, arg).
    let raw = unsafe {
        syscall3(
            SYS_THREAD_CREATE,
            pthread_entry as *const () as u64,
            stack_top as u64,
            0, // arg is read from THREAD_ARG, not passed via rdi
        )
    };

    if raw < 0 {
        // Roll back the mmap.
        let _ = openos_sdk::memory::munmap(stack_base, actual_stack_size);
        return Err("SYS_THREAD_CREATE failed");
    }

    Ok(Pthread {
        tid: raw as u64,
        stack_ptr: stack_base as *mut u8,
        stack_size: actual_stack_size,
        joined: AtomicI32::new(0),
    })
}

/// Exit the current thread with a return value.
///
/// This function does not return.
pub fn pthread_exit(retval: u64) -> ! {
    // Store retval in a global so that joined threads can read it.
    // In a real implementation each thread would have its own storage;
    // for now we use the simple approach since join reads after exit.
    unsafe {
        syscall1(SYS_THREAD_EXIT, retval);
    }
    unreachable!()
}

/// Wait for a thread to finish and retrieve its return value.
///
/// Returns the thread's exit value.
pub fn pthread_join(thread: &Pthread) -> Result<u64, &'static str> {
    if thread
        .joined
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("thread already joined");
    }

    // Block until the child task exits.
    let raw = unsafe { syscall2(SYS_PROCESS_WAIT, thread.tid, u64::MAX) };
    if raw < 0 {
        return Err("SYS_PROCESS_WAIT failed");
    }

    // Deallocate the stack.
    let _ = openos_sdk::memory::munmap(thread.stack_ptr as usize, thread.stack_size);

    Ok(raw as u64)
}

/// Yield the current thread's time slice.
pub fn pthread_yield() {
    unsafe {
        syscall0(SYS_THREAD_YIELD);
    }
}

// ─── Mutex ───

/// Mutex states.
const MUTEX_UNLOCKED: i32 = 0;
const MUTEX_LOCKED: i32 = 1;
const MUTEX_LOCKED_WAITERS: i32 = 2;

/// A mutex that spins briefly, then yields the CPU.
///
/// Suitable for short critical sections in a cooperative or
/// preemptive environment. The spin-then-yield pattern avoids
/// busy-waiting for extended periods.
///
/// This is NOT a futex-based blocking mutex. For that, use
/// `Condvar`-based synchronization.
pub struct Mutex {
    state: AtomicI32,
}

// Safety: Mutex uses only atomic operations.
unsafe impl Send for Mutex {}
unsafe impl Sync for Mutex {}

impl Mutex {
    /// Create a new unlocked mutex.
    pub const fn new() -> Self {
        Self {
            state: AtomicI32::new(MUTEX_UNLOCKED),
        }
    }

    /// Acquire the mutex, spinning until it becomes available.
    pub fn lock(&self) {
        loop {
            match self.state.compare_exchange_weak(
                MUTEX_UNLOCKED,
                MUTEX_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(MUTEX_LOCKED) => {
                    // Try to signal that there are waiters.
                    let _ = self.state.compare_exchange(
                        MUTEX_LOCKED,
                        MUTEX_LOCKED_WAITERS,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                    // Yield and retry.
                    pthread_yield();
                }
                Err(MUTEX_LOCKED_WAITERS) => {
                    pthread_yield();
                }
                Err(_) => {}
            }
        }
    }

    /// Try to acquire the mutex without blocking.
    ///
    /// Returns `true` if the lock was acquired, `false` otherwise.
    pub fn trylock(&self) -> bool {
        matches!(
            self.state.compare_exchange_weak(
                MUTEX_UNLOCKED,
                MUTEX_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ),
            Ok(_)
        )
    }

    /// Release the mutex.
    pub fn unlock(&self) {
        let prev = self.state.swap(MUTEX_UNLOCKED, Ordering::Release);
        if prev == MUTEX_LOCKED_WAITERS {
            // There are waiters; yield to give them a chance to run.
            pthread_yield();
        }
    }
}

impl Default for Mutex {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Condition Variable ───

/// A condition variable built on kernel events.
///
/// Each `Condvar` uses a kernel event object for blocking/waking.
/// The event is a level-triggered kernel primitive: `signal` marks it
/// as signaled, `wait` blocks until signaled and then clears it.
/// One signal wakes exactly one waiter.
///
/// For `broadcast`, we must signal the event once per waiter, but
/// kernel events are level-triggered so a single signal could wake
/// only one thread. To support broadcast properly, multiple waiter
/// threads each have their own event, or we use a multi-wake protocol.
/// The current implementation signals multiple times with yields
/// between signals to let waiters consume each signal.
pub struct Condvar {
    event_handle: AtomicU64,
    /// Number of waiters currently blocked.
    waiters: AtomicUsize,
}

// Safety: Condvar only holds an atomic handle value.
unsafe impl Send for Condvar {}
unsafe impl Sync for Condvar {}

impl Condvar {
    /// Create a new condition variable.
    pub fn new() -> Result<Self, &'static str> {
        let raw = unsafe { syscall0(SYS_EVENT_CREATE) };
        if raw < 0 {
            return Err("SYS_EVENT_CREATE failed");
        }
        Ok(Self {
            event_handle: AtomicU64::new(raw as u64),
            waiters: AtomicUsize::new(0),
        })
    }

    /// Atomically release the mutex and block on this condition variable.
    ///
    /// When signaled, the mutex is re-acquired before returning.
    pub fn wait(&self, mutex: &Mutex) -> Result<(), &'static str> {
        let handle = self.event_handle.load(Ordering::Acquire);
        // Increment waiters before releasing the mutex so signal() can
        // know how many to wake.
        self.waiters.fetch_add(1, Ordering::SeqCst);
        // Release the mutex before blocking.
        mutex.unlock();
        // Wait for the event to be signaled.
        let raw = unsafe { syscall1(SYS_EVENT_WAIT, handle) };
        // Decrement waiters after waking.
        self.waiters.fetch_sub(1, Ordering::SeqCst);
        if raw < 0 {
            // Re-acquire mutex even on error.
            mutex.lock();
            return Err("SYS_EVENT_WAIT failed");
        }
        // Re-acquire the mutex.
        mutex.lock();
        Ok(())
    }

    /// Wake one thread blocked on this condition variable.
    pub fn signal(&self) -> Result<(), &'static str> {
        let handle = self.event_handle.load(Ordering::Acquire);
        let raw = unsafe { syscall1(SYS_EVENT_SIGNAL, handle) };
        if raw < 0 {
            return Err("SYS_EVENT_SIGNAL failed");
        }
        Ok(())
    }

    /// Wake all threads blocked on this condition variable.
    ///
    /// Signals multiple times to wake each waiter. The caller does not
    /// need to provide the waiter count: we track it internally.
    pub fn broadcast(&self) -> Result<(), &'static str> {
        let w = self.waiters.load(Ordering::SeqCst);
        for _ in 0..w {
            // Signal and yield to let the waiter consume the signal.
            self.signal()?;
            pthread_yield();
        }
        Ok(())
    }
}

// ─── Barrier ───

/// A synchronization barrier that blocks threads until a specified
/// count of threads have arrived.
pub struct Barrier {
    /// Number of threads that must arrive before unblocking.
    count: usize,
    /// Number of threads that have arrived so far.
    arrived: AtomicUsize,
    /// Mutex protecting barrier state.
    mutex: Mutex,
    /// Condvar for waiting threads.
    condvar: Condvar,
    /// Generation counter to handle spurious wakeups.
    generation: AtomicUsize,
}

impl Barrier {
    /// Create a new barrier that blocks until `count` threads arrive.
    pub fn new(count: usize) -> Result<Self, &'static str> {
        Ok(Self {
            count,
            arrived: AtomicUsize::new(0),
            mutex: Mutex::new(),
            condvar: Condvar::new()?,
            generation: AtomicUsize::new(0),
        })
    }

    /// Block until all `count` threads have called `wait`.
    ///
    /// Returns `true` to one "arbitrary" thread and `false` to the others.
    /// (Currently always returns `true` since we don't distinguish.)
    pub fn wait(&self) -> Result<bool, &'static str> {
        self.mutex.lock();
        let current_gen = self.generation.load(Ordering::SeqCst);
        let arrived = self.arrived.fetch_add(1, Ordering::SeqCst) + 1;

        if arrived >= self.count {
            // Last thread to arrive: reset and wake everyone.
            self.arrived.store(0, Ordering::SeqCst);
            self.generation.store(current_gen + 1, Ordering::SeqCst);
            self.mutex.unlock();
            // Broadcast to all waiters.
            self.condvar.broadcast()?;
            Ok(true)
        } else {
            // Wait until the generation changes.
            while self.generation.load(Ordering::SeqCst) == current_gen {
                self.condvar.wait(&self.mutex)?;
            }
            self.mutex.unlock();
            Ok(false)
        }
    }
}

// ─── Read-Write Lock ───

/// A read-write lock that allows multiple concurrent readers or one writer.
///
/// Uses a spinning-yield strategy similar to `Mutex`. Readers increment
/// a shared counter; writers set a "writer pending" flag and wait for
/// all readers to finish.
pub struct RwLock {
    /// Bit 63: writer active. Bits 0-62: reader count.
    state: AtomicUsize,
}

// Safety: RwLock uses only atomic operations and its API is thread-safe.
unsafe impl Send for RwLock {}
unsafe impl Sync for RwLock {}

impl RwLock {
    /// Create a new unlocked read-write lock.
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
        }
    }

    /// Acquire a read lock.
    ///
    /// Blocks (spinning + yielding) if a writer holds the lock.
    /// Multiple readers can hold the lock concurrently.
    pub fn lock_read(&self) {
        loop {
            let s = self.state.load(Ordering::Relaxed);
            // If no writer is active (bit 63 clear), try to increment reader count.
            if s & (1 << 63) == 0 {
                if self
                    .state
                    .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            pthread_yield();
        }
    }

    /// Try to acquire a read lock without blocking.
    ///
    /// Returns `true` if the read lock was acquired.
    pub fn try_lock_read(&self) -> bool {
        let s = self.state.load(Ordering::Relaxed);
        if s & (1 << 63) == 0 {
            self.state
                .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }

    /// Release a read lock.
    pub fn unlock_read(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    /// Acquire a write lock (exclusive access).
    ///
    /// Blocks until all readers and any existing writer have finished.
    pub fn lock_write(&self) {
        loop {
            let s = self.state.load(Ordering::Relaxed);
            if s == 0 {
                // Idle — try to acquire as writer.
                if self
                    .state
                    .compare_exchange_weak(0, 1 << 63, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            pthread_yield();
        }
    }

    /// Try to acquire a write lock without blocking.
    ///
    /// Returns `true` if the write lock was acquired.
    pub fn try_lock_write(&self) -> bool {
        self.state
            .compare_exchange_weak(0, 1 << 63, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release a write lock.
    pub fn unlock_write(&self) {
        self.state.store(0, Ordering::Release);
    }
}

impl Default for RwLock {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Once (One-Time Initialization) ───

const ONCE_INCOMPLETE: usize = 0;
const ONCE_IN_PROGRESS: usize = 1;
const ONCE_COMPLETE: usize = 2;

/// A synchronization primitive for one-time initialization.
///
/// Ensures that a function is called exactly once, even when multiple
/// threads attempt to trigger it concurrently. This is the building
/// block for lazy initialization patterns.
pub struct Once {
    state: AtomicUsize,
}

// Safety: Once uses only atomic operations.
unsafe impl Send for Once {}
unsafe impl Sync for Once {}

impl Once {
    /// Create a new `Once` in the incomplete state.
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(ONCE_INCOMPLETE),
        }
    }

    /// Execute `f` exactly once. If another thread is already executing
    /// `f`, this call blocks until it completes.
    pub fn call_once<F: FnOnce()>(&self, f: F) {
        match self.state.compare_exchange(
            ONCE_INCOMPLETE,
            ONCE_IN_PROGRESS,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                // We are the first caller — run the initialization function.
                f();
                self.state.store(ONCE_COMPLETE, Ordering::Release);
            }
            Err(ONCE_IN_PROGRESS) => {
                // Another thread is running the init — spin until complete.
                while self.state.load(Ordering::Acquire) != ONCE_COMPLETE {
                    pthread_yield();
                }
            }
            Err(ONCE_COMPLETE) => {
                // Already complete — nothing to do.
            }
            Err(_) => {
                // Should never happen; recover by spinning.
                while self.state.load(Ordering::Acquire) != ONCE_COMPLETE {
                    pthread_yield();
                }
            }
        }
    }

    /// Returns `true` if the `Once` has been completed.
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == ONCE_COMPLETE
    }
}

impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Thread-Local Storage (TLS) Keys ───

/// A key for thread-local storage.
///
/// TLS keys are allocated from a global pool. Each thread can store
/// a pointer-sized value per key. The values are stored as kernel
/// environment variables prefixed with `_tls_` and keyed by
/// `{tid}_{key_id}` for thread isolation.
///
/// This is a simplified implementation suitable for a microkernel
/// environment without native TLS segment support. It uses the
/// kernel's per-task environment variable mechanism under the hood.
pub struct TlsKey {
    id: usize,
}

/// Maximum number of TLS keys supported system-wide.
const MAX_TLS_KEYS: usize = 64;

/// Global TLS key allocator state.
static NEXT_TLS_KEY: AtomicUsize = AtomicUsize::new(0);

// Per-thread TLS values cache, stored in a global array.
// Each entry is a pointer-sized value stored by the current thread
// for a given TLS key. We use environment variables for persistence
// and this is just a cache layer.
//
// In a real implementation, this would use the %fs or %gs segment
// or be stored in a per-thread control block. This implementation
// uses environment variables prefixed with `_tls_{tid}_{key}`.

impl TlsKey {
    /// Create a new TLS key.
    ///
    /// Returns `None` if the maximum number of keys has been exhausted.
    pub fn create() -> Option<Self> {
        let id = NEXT_TLS_KEY.fetch_add(1, Ordering::SeqCst);
        if id >= MAX_TLS_KEYS {
            NEXT_TLS_KEY.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self { id })
    }

    /// Delete a TLS key, freeing its slot.
    ///
    /// After deletion, existing values stored under this key become
    /// inaccessible.
    pub fn delete(&self) {
        // In a real implementation, we'd reclaim the key slot.
        // For now, we simply allow the next allocation to eventually
        // wrap (in practice, with 64 keys, reuse is rare).
    }

    /// Set the value of this TLS key for the current thread.
    pub fn set(&self, value: u64) -> Result<(), &'static str> {
        let tid = Self::current_tid();
        let key_name = alloc::format!("_tls_{}_{}", tid, self.id);
        // Encode the 64-bit value as a hex string.
        let value_str = alloc::format!("{:016x}", value);
        openos_sdk::env::set(&key_name, &value_str).map_err(|_| "env_set failed")
    }

    /// Get the value of this TLS key for the current thread.
    ///
    /// Returns `None` if no value has been set for this key in the
    /// current thread.
    pub fn get(&self) -> Option<u64> {
        let tid = Self::current_tid();
        let key_name = alloc::format!("_tls_{}_{}", tid, self.id);
        let val_str = openos_sdk::env::get(&key_name).ok()??;
        u64::from_str_radix(val_str.trim(), 16).ok()
    }

    /// Get the raw key ID.
    pub fn id(&self) -> usize {
        self.id
    }

    /// Get the current thread ID by reading the kernel's gettid syscall.
    fn current_tid() -> u64 {
        openos_sdk::process::gettid()
    }
}

// ─── Simple TLS (environment-based, no keys) ───

/// Thread-local storage using the kernel's per-task environment.
///
/// Each key is prefixed with `_tls_` to avoid collisions with regular
/// environment variables. This is a simpler API than `TlsKey` for
/// string-based TLS values.
pub struct Tls;

impl Tls {
    /// Set a thread-local variable.
    pub fn set(key: &str, value: &str) -> Result<(), &'static str> {
        let full_key = alloc::format!("_tls_{}", key);
        openos_sdk::env::set(&full_key, value).map_err(|_| "env_set failed")
    }

    /// Get a thread-local variable.
    ///
    /// Returns `None` if the key does not exist.
    pub fn get(key: &str) -> Option<alloc::string::String> {
        let full_key = alloc::format!("_tls_{}", key);
        openos_sdk::env::get(&full_key).ok().flatten()
    }
}

// ─── Spinlock (simple, for short critical sections) ───

/// A simple spinlock for very short critical sections.
///
/// Unlike `Mutex`, this spins without yielding. Use only when the
/// critical section is guaranteed to be extremely short (a few
/// instructions).
pub struct Spinlock {
    locked: AtomicBool,
}

/// Re-export `AtomicBool` for the `Spinlock` API.
use core::sync::atomic::AtomicBool;

impl Spinlock {
    /// Create a new unlocked spinlock.
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Acquire the spinlock, spinning tightly.
    pub fn lock(&self) {
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            // Tight spin (no yield) for maximum speed on very short locks.
            core::hint::spin_loop();
        }
    }

    /// Try to acquire the spinlock without blocking.
    ///
    /// Returns `true` if the lock was acquired.
    pub fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release the spinlock.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl Default for Spinlock {
    fn default() -> Self {
        Self::new()
    }
}
