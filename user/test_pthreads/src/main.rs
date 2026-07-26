//! Test program for the pthreads library.
//!
//! Exercises: thread create/join, mutex, condition variable, barrier, TLS.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use openos_sdk::console;
use pthreads::{pthread_create, pthread_exit, pthread_join, Barrier, Condvar, Mutex, Pthread, Tls};

// ─── Bump allocator for user-space (128 KiB heap) ───

struct BumpAllocator {
    heap: UnsafeCell<[u8; 131072]>,
    offset: core::cell::Cell<usize>,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut off = self.offset.get();
        off = (off + align - 1) & !(align - 1);
        if off + size > 131072 {
            return core::ptr::null_mut();
        }
        let ptr = (*self.heap.get()).as_mut_ptr().add(off);
        self.offset.set(off + size);
        ptr
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: no-op dealloc.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: UnsafeCell::new([0u8; 131072]),
    offset: core::cell::Cell::new(0),
};

// ─── Panic handler ───

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in test_pthreads!");
    openos_sdk::process::exit(1);
}

/// Helper: print pass/fail.
fn check(name: &str, ok: bool) {
    if ok {
        let _ = console::write("  PASS: ");
    } else {
        let _ = console::write("  FAIL: ");
    }
    let _ = console::writeln(name);
}

// ─── Shared state for counter test ───

struct SharedState {
    counter: UnsafeCell<u64>,
    lock: Mutex,
}

// Safety: counter is only accessed under the mutex.
unsafe impl Sync for SharedState {}

static SHARED: SharedState = SharedState {
    counter: UnsafeCell::new(0),
    lock: Mutex::new(),
};

/// Worker: increments the shared counter 1000 times under the mutex.
fn counter_worker(_arg: *mut u8) {
    for _ in 0..1000 {
        SHARED.lock.lock();
        // Safety: we hold the lock.
        unsafe {
            *SHARED.counter.get() += 1;
        }
        SHARED.lock.unlock();
    }
    pthread_exit(0);
}

/// Test 1: create 4 threads, each increment counter 1000 times, verify 4000.
fn test_counter() {
    let _ = console::writeln("[Test 1] Mutex + counter (4 threads x 1000 increments)");

    let mut threads: [Option<Pthread>; 4] = [None, None, None, None];
    for i in 0..4 {
        threads[i] = Some(
            pthread_create(counter_worker, core::ptr::null_mut(), 0)
                .expect("pthread_create failed"),
        );
    }

    for thread in threads.iter() {
        if let Some(t) = thread {
            let _ = pthread_join(t);
        }
    }

    // Safety: all threads have joined, no concurrent access.
    let final_val = unsafe { *SHARED.counter.get() };
    check("counter == 4000", final_val == 4000);
}

// ─── Condition variable test ───

static CV_MUTEX: Mutex = Mutex::new();
// Condvar initialized at runtime (not const). Use a wrapper with AtomicBool.
struct LateCondvar {
    inner: UnsafeCell<Option<Condvar>>,
    ready: AtomicBool,
}

unsafe impl Sync for LateCondvar {}

static CV_CONDVAR: LateCondvar = LateCondvar {
    inner: UnsafeCell::new(None),
    ready: AtomicBool::new(false),
};

fn cv_init() {
    let cv = Condvar::new().expect("Condvar::new failed");
    // Safety: called once at startup before any threads use it.
    unsafe {
        *CV_CONDVAR.inner.get() = Some(cv);
    }
    CV_CONDVAR.ready.store(true, Ordering::Release);
}

fn cv_get() -> &'static Condvar {
    while !CV_CONDVAR.ready.load(Ordering::Acquire) {
        // spin
    }
    // Safety: initialized and ready.
    unsafe { (*CV_CONDVAR.inner.get()).as_ref().unwrap() }
}

static CV_READY: AtomicU64 = AtomicU64::new(0);

fn cv_waiter(_arg: *mut u8) {
    let cv = cv_get();
    CV_MUTEX.lock();
    while CV_READY.load(Ordering::SeqCst) == 0 {
        let _ = cv.wait(&CV_MUTEX);
    }
    CV_READY.store(2, Ordering::SeqCst);
    CV_MUTEX.unlock();
    pthread_exit(0);
}

fn test_condvar() {
    let _ = console::writeln("[Test 2] Condition variable signaling");

    cv_init();
    let cv = cv_get();

    let waiter = pthread_create(cv_waiter, core::ptr::null_mut(), 0)
        .expect("pthread_create for condvar waiter failed");

    // Give the waiter time to block.
    openos_sdk::time::sleep(5);

    // Signal the waiter.
    CV_READY.store(1, Ordering::SeqCst);
    let _ = cv.signal();

    let _ = pthread_join(&waiter);

    check(
        "condvar waiter completed",
        CV_READY.load(Ordering::SeqCst) == 2,
    );
}

// ─── Barrier test ───

static BARRIER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn barrier_worker(arg: *mut u8) {
    let barrier_ptr = arg as *const Barrier;
    // Safety: barrier lives on the stack of test_barrier and outlives all threads.
    let barrier = unsafe { &*barrier_ptr };

    // Increment before barrier.
    BARRIER_COUNTER.fetch_add(1, Ordering::SeqCst);

    // Wait for all threads.
    let _ = barrier.wait();

    // After barrier: all threads have incremented.
    BARRIER_COUNTER.fetch_add(10, Ordering::SeqCst);

    pthread_exit(0);
}

fn test_barrier() {
    let _ = console::writeln("[Test 3] Barrier synchronization (3 threads)");

    let barrier = Barrier::new(3).expect("barrier creation failed");
    let barrier_ptr = &barrier as *const Barrier;

    let mut threads: [Option<Pthread>; 3] = [None, None, None];
    for i in 0..3 {
        threads[i] = Some(
            pthread_create(barrier_worker, barrier_ptr as *mut u8, 0)
                .expect("pthread_create for barrier failed"),
        );
    }

    for thread in threads.iter() {
        if let Some(t) = thread {
            let _ = pthread_join(t);
        }
    }

    // 3 threads each: +1 before barrier, +10 after = 33 total.
    check(
        "barrier counter == 33",
        BARRIER_COUNTER.load(Ordering::SeqCst) == 33,
    );
}

// ─── TLS test ───

fn tls_worker(_arg: *mut u8) {
    Tls::set("mykey", "hello_from_thread").ok();
    let val = Tls::get("mykey");
    let ok = val.as_deref() == Some("hello_from_thread");
    check("TLS set/get in thread", ok);
    pthread_exit(0);
}

fn test_tls() {
    let _ = console::writeln("[Test 4] Thread-local storage via env");

    let t = pthread_create(tls_worker, core::ptr::null_mut(), 0)
        .expect("pthread_create for TLS test failed");

    let _ = pthread_join(&t);
}

// ─── Entry point ───

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("=== pthreads Integration Test ===");

    test_counter();
    test_condvar();
    test_barrier();
    test_tls();

    let _ = console::writeln("=== All pthreads tests complete ===");
    openos_sdk::process::exit(0);
}
