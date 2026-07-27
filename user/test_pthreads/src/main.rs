//! Test program for the pthreads library.
//!
//! Exercises: thread create/join, mutex, condition variable, barrier,
//! read-write lock, one-time initialization, TLS keys, spinlock, TLS.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use openos_sdk::console;
use pthreads::{
    pthread_create, pthread_exit, pthread_join, Barrier, Condvar, Mutex, Once, Pthread, RwLock,
    Spinlock, Tls, TlsKey,
};

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

struct LateCondvar {
    inner: UnsafeCell<Option<Condvar>>,
}

unsafe impl Sync for LateCondvar {}

static CV_CONDVAR: LateCondvar = LateCondvar {
    inner: UnsafeCell::new(None),
};

static CV_READY: AtomicU64 = AtomicU64::new(0);

fn cv_init() {
    let cv = Condvar::new().expect("Condvar::new failed");
    // Safety: called once at startup before any threads use it.
    unsafe {
        *CV_CONDVAR.inner.get() = Some(cv);
    }
}

fn cv_get() -> &'static Condvar {
    // Safety: initialized once before threads run.
    unsafe { (*CV_CONDVAR.inner.get()).as_ref().unwrap() }
}

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

// ─── Read-Write Lock test ───

static RW_COUNTER: AtomicU64 = AtomicU64::new(0);
static RW_LOCK: RwLock = RwLock::new();

fn rw_reader(_arg: *mut u8) {
    for _ in 0..50 {
        RW_LOCK.lock_read();
        // Reading is safe with multiple concurrent readers.
        let _val = RW_COUNTER.load(Ordering::SeqCst);
        RW_LOCK.unlock_read();
    }
    pthread_exit(0);
}

fn rw_writer(_arg: *mut u8) {
    for _ in 0..50 {
        RW_LOCK.lock_write();
        // Critical section: only one writer at a time.
        let val = RW_COUNTER.load(Ordering::SeqCst);
        RW_COUNTER.store(val + 1, Ordering::SeqCst);
        RW_LOCK.unlock_write();
    }
    pthread_exit(0);
}

fn test_rwlock() {
    let _ = console::writeln("[Test 4] Read-Write Lock (readers + writers)");

    let mut threads: [Option<Pthread>; 6] = [None, None, None, None, None, None];
    // 4 readers.
    for i in 0..4 {
        threads[i] = Some(
            pthread_create(rw_reader, core::ptr::null_mut(), 0)
                .expect("pthread_create for rw_reader failed"),
        );
    }
    // 2 writers (each does 50 increments = 100 total).
    for i in 4..6 {
        threads[i] = Some(
            pthread_create(rw_writer, core::ptr::null_mut(), 0)
                .expect("pthread_create for rw_writer failed"),
        );
    }

    for thread in threads.iter() {
        if let Some(t) = thread {
            let _ = pthread_join(t);
        }
    }

    let final_val = RW_COUNTER.load(Ordering::SeqCst);
    check("rw counter == 100", final_val == 100);
}

// ─── Once test ───

static ONCE: Once = Once::new();
static ONCE_VALUE: AtomicU64 = AtomicU64::new(0);

fn once_worker(_arg: *mut u8) {
    ONCE.call_once(|| {
        ONCE_VALUE.store(42, Ordering::SeqCst);
    });
    // After call_once, the value must be 42.
    let val = ONCE_VALUE.load(Ordering::SeqCst);
    check("once value is 42", val == 42);
    pthread_exit(0);
}

fn test_once() {
    let _ = console::writeln("[Test 5] One-time initialization (Once)");

    // Call once from the main thread.
    ONCE.call_once(|| {
        ONCE_VALUE.store(42, Ordering::SeqCst);
    });
    check("once completed (main)", ONCE.is_completed());

    // Spawn a thread that also calls call_once (should see 42, not re-execute).
    let t = pthread_create(once_worker, core::ptr::null_mut(), 0)
        .expect("pthread_create for once test failed");
    let _ = pthread_join(&t);
}

// ─── TlsKey test ───

fn tls_key_worker(_arg: *mut u8) {
    // Create a new key and set a value on it from this thread.
    let my_key = TlsKey::create().expect("TlsKey::create failed");
    let ok1 = my_key.set(99).is_ok();
    let val = my_key.get();
    let ok2 = val == Some(99);
    check("TLS key set/get in thread", ok1 && ok2);

    my_key.delete();
    pthread_exit(0);
}

fn test_tls_key() {
    let _ = console::writeln("[Test 6] TlsKey create/set/get/delete");

    let key = TlsKey::create().expect("TlsKey::create failed");

    // Set from main thread.
    let _ = key.set(42);
    let val = key.get();
    check("TLS key set/get in main", val == Some(42));

    // Set from spawned thread (isolated storage).
    let t = pthread_create(tls_key_worker, core::ptr::null_mut(), 0)
        .expect("pthread_create for TLS key test failed");
    let _ = pthread_join(&t);

    // Main thread's value should still be intact.
    let val = key.get();
    check("TLS key isolated per-thread", val == Some(42));
}

// ─── Simple TLS test (env-based) ───

fn tls_env_worker(_arg: *mut u8) {
    Tls::set("mykey", "hello_from_thread").ok();
    let val = Tls::get("mykey");
    let ok = val.as_deref() == Some("hello_from_thread");
    check("TLS env set/get in thread", ok);
    pthread_exit(0);
}

fn test_tls_env() {
    let _ = console::writeln("[Test 7] Thread-local storage via env");

    let t = pthread_create(tls_env_worker, core::ptr::null_mut(), 0)
        .expect("pthread_create for TLS test failed");
    let _ = pthread_join(&t);
}

// ─── Spinlock test ───

static SPIN_COUNTER: AtomicU64 = AtomicU64::new(0);
static SPIN_LOCK: Spinlock = Spinlock::new();

fn spin_worker(_arg: *mut u8) {
    for _ in 0..200 {
        SPIN_LOCK.lock();
        let val = SPIN_COUNTER.load(Ordering::SeqCst);
        SPIN_COUNTER.store(val + 1, Ordering::SeqCst);
        SPIN_LOCK.unlock();
    }
    pthread_exit(0);
}

fn test_spinlock() {
    let _ = console::writeln("[Test 8] Spinlock (2 threads x 200 increments)");

    let mut threads: [Option<Pthread>; 2] = [None, None];
    for i in 0..2 {
        threads[i] = Some(
            pthread_create(spin_worker, core::ptr::null_mut(), 0)
                .expect("pthread_create for spinlock test failed"),
        );
    }

    for thread in threads.iter() {
        if let Some(t) = thread {
            let _ = pthread_join(t);
        }
    }

    let final_val = SPIN_COUNTER.load(Ordering::SeqCst);
    check("spin counter == 400", final_val == 400);
}

// ─── Entry point ───

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("=== pthreads Integration Test ===");

    test_counter();
    test_condvar();
    test_barrier();
    test_rwlock();
    test_once();
    test_tls_key();
    test_tls_env();
    test_spinlock();

    let _ = console::writeln("=== All pthreads tests complete ===");
    openos_sdk::process::exit(0);
}
