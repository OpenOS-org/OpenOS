//! Test program for the pthreads library.
//!
//! Exercises: thread create/join, mutex, condition variable, barrier, TLS.

#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use openos_sdk::console;
use pthreads::{Barrier, Condvar, Mutex, Pthread, Tls, pthread_create, pthread_exit, pthread_join};

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
    counter: u64,
    lock: Mutex,
    done: AtomicU64,
}

static SHARED: SharedState = SharedState {
    counter: 0,
    lock: Mutex::new(),
    done: AtomicU64::new(0),
};

/// Worker: increments the shared counter 1000 times under the mutex.
fn counter_worker(_arg: *mut u8) {
    for _ in 0..1000 {
        SHARED.lock.lock();
        SHARED.counter += 1;
        SHARED.lock.unlock();
    }
    SHARED.done.fetch_add(1, Ordering::SeqCst);
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

    check("counter == 4000", SHARED.counter == 4000);
}

// ─── Condition variable test ───

static CV_MUTEX: Mutex = Mutex::new();
static CV_CONDVAR: Condvar = match Condvar::new() {
    Ok(c) => c,
    Err(_) => panic!("Condvar init failed"),
};
static CV_READY: AtomicU64 = AtomicU64::new(0);

fn cv_waiter(_arg: *mut u8) {
    CV_MUTEX.lock();
    while CV_READY.load(Ordering::SeqCst) == 0 {
        let _ = CV_CONDVAR.wait(&CV_MUTEX);
    }
    CV_READY.store(2, Ordering::SeqCst);
    CV_MUTEX.unlock();
    pthread_exit(0);
}

fn test_condvar() {
    let _ = console::writeln("[Test 2] Condition variable signaling");

    let waiter = pthread_create(cv_waiter, core::ptr::null_mut(), 0)
        .expect("pthread_create for condvar waiter failed");

    // Give the waiter time to block.
    openos_sdk::time::sleep(5);

    // Signal the waiter.
    CV_READY.store(1, Ordering::SeqCst);
    let _ = CV_CONDVAR.signal();

    let _ = pthread_join(&waiter);

    check("condvar waiter completed", CV_READY.load(Ordering::SeqCst) == 2);
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
    check("barrier counter == 33", BARRIER_COUNTER.load(Ordering::SeqCst) == 33);
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
