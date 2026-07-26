//! POSIX-like threading library for OpenOS.
//!
//! Provides thread creation/join/exit, mutexes, condition variables,
//! barriers, and thread-local storage built on top of OpenOS syscalls.
//!
//! # Example
//! ```no_run
//! use pthreads::{Pthread, Mutex};
//!
//! let mut lock = Mutex::new();
//! lock.lock();
//! // critical section
//! lock.unlock();
//! ```

#![no_std]

extern crate alloc;

use core::arch::asm;
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

// ─── Syscall numbers (must match kernel/src/syscall/number.rs) ───

const SYS_THREAD_CREATE: u64 = 0x40;
const SYS_THREAD_EXIT: u64 = 0x41;
const SYS_THREAD_YIELD: u64 = 0x42;
const SYS_PROCESS_WAIT: u64 = 0x33;
const SYS_SLEEP: u64 = 0xF1;
const SYS_EVENT_CREATE: u64 = 0xF2;
const SYS_EVENT_SIGNAL: u64 = 0xF3;
const SYS_EVENT_WAIT: u64 = 0xFB;
const SYS_EVENT_DESTROY: u64 = 0xFC;
const SYS_CONSOLE_WRITE: u64 = 0xF0;

// ─── Raw syscall wrappers ───

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
    /// Exit value written by `pthread_exit`.
    retval: AtomicU64,
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
        retval: AtomicU64::new(0),
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
    if thread.joined.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
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
        syscall1(SYS_THREAD_YIELD, 0);
    }
}

// ─── Mutex ───

/// Mutex states.
const MUTEX_UNLOCKED: i32 = 0;
const MUTEX_LOCKED: i32 = 1;
const MUTEX_LOCKED_WAITERS: i32 = 2;

/// A simple spin-lock mutex using atomic CAS.
///
/// This is suitable for short critical sections. For longer waits
/// consider using condition variables.
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
        if self.state.swap(MUTEX_UNLOCKED, Ordering::Release) == MUTEX_LOCKED_WAITERS {
            // There are waiters; they will notice UNLOCKED on next iteration.
            // A futex-like wake would be ideal but we have no FUTEX syscall.
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
/// Broadcast wakes all waiters by signaling the event once (all
/// waiters are on the same kernel event).
pub struct Condvar {
    event_handle: AtomicU64,
}

// Safety: Condvar only holds an atomic handle value.
unsafe impl Send for Condvar {}
unsafe impl Sync for Condvar {}

impl Condvar {
    /// Create a new condition variable.
    pub fn new() -> Result<Self, &'static str> {
        let raw = unsafe { syscall1(SYS_EVENT_CREATE, 0) };
        if raw < 0 {
            return Err("SYS_EVENT_CREATE failed");
        }
        Ok(Self {
            event_handle: AtomicU64::new(raw as u64),
        })
    }

    /// Atomically release the mutex and block on this condition variable.
    ///
    /// When signaled, the mutex is re-acquired before returning.
    pub fn wait(&self, mutex: &Mutex) -> Result<(), &'static str> {
        let handle = self.event_handle.load(Ordering::Acquire);
        // Release the mutex before blocking.
        mutex.unlock();
        // Wait for the event to be signaled.
        let raw = unsafe { syscall1(SYS_EVENT_WAIT, handle) };
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
    ///
    /// Since we use a single kernel event, `signal` wakes exactly one
    /// waiter (level-triggered, cleared after first wake).
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
    /// Issues the signal multiple times to wake each waiter.
    /// The caller should ensure `wait_count` is accurate.
    pub fn broadcast(&self, wait_count: usize) -> Result<(), &'static str> {
        for _ in 0..wait_count {
            self.signal()?;
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
        let gen = self.arrived.load(Ordering::SeqCst);
        let current_gen = self.generation.load(Ordering::SeqCst);
        let arrived = self.arrived.fetch_add(1, Ordering::SeqCst) + 1;

        if arrived >= self.count {
            // Last thread to arrive: reset and wake everyone.
            self.arrived.store(0, Ordering::SeqCst);
            self.generation.store(current_gen + 1, Ordering::SeqCst);
            self.mutex.unlock();
            // Broadcast to all waiters (count - 1 threads are waiting).
            let _ = self.condvar.broadcast(self.count - 1);
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

// ─── Thread-Local Storage (via environment variables) ───

/// Thread-local storage using the kernel's per-task environment.
///
/// Each key is prefixed with `_tls_` to avoid collisions with regular
/// environment variables.
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
