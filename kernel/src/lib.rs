//! `OpenOS` kernel library crate.
//!
//! This `lib.rs` enables `cargo test` on the host target. When compiling
//! normally (binary via `main.rs`), `#![no_std]` and `#![no_main]` are
//! set there. When compiling for tests, this crate is built with `std`
//! so the standard test harness works.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![feature(abi_x86_interrupt)]
#![cfg_attr(not(test), feature(alloc_error_handler))]
#![allow(unused_features)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![allow(
    clippy::module_inception,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    dead_code,
    unused_imports,
    unused_variables,
    clippy::missing_const_for_fn,
    clippy::used_underscore_items,
    clippy::result_unit_err,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod elf;
pub mod frame_alloc;
pub mod fs;
pub mod handle;
pub mod initrd;
pub mod ipc;
pub mod memory;
pub mod module;
pub mod net;
pub mod sync;
pub mod syscall;
pub mod task;

/// Global ramdisk data. Set once during boot, read-only thereafter.
/// Used by `process_start` to load ELF binaries from the initrd.
pub static mut RAMDISK_DATA: Option<&'static [u8]> = None;

/// Serial test lock for tests that share global static state across modules.
///
/// `cargo test` runs tests from different modules in parallel by default.
/// Tests that mutate global statics (e.g., `FILE_LOCKS`, `MOUNT_TABLE`,
/// `ARP_TABLE`, `ROUTING_TABLE`) can interfere with each other across
/// module boundaries. Acquire this lock at the start of such tests to
/// ensure exclusive access to all global mutable state.
///
/// Uses an atomic spinlock rather than `std::sync::Mutex` to avoid
/// pthread destructor ordering issues at process exit (SIGSEGV).
///
/// # Usage
///
/// ```ignore
/// let _guard = crate::TEST_SERIAL_LOCK.lock();
/// ```
///
/// The lock is released automatically when `_guard` is dropped (at the
/// end of the test or when explicitly dropped).
#[cfg(test)]
mod test_serial_lock {
    use core::hint;
    use core::sync::atomic::{AtomicBool, Ordering};

    /// A simple spinlock that wraps an `AtomicBool`.
    ///
    /// Unlike `std::sync::Mutex`, this has no destructor, avoiding
    /// SIGSEGV at process exit when the test binary unloads.
    pub struct SerialLock {
        locked: AtomicBool,
    }

    impl SerialLock {
        /// Create a new unlocked serial lock.
        pub const fn new() -> Self {
            Self {
                locked: AtomicBool::new(false),
            }
        }

        /// Acquire the lock, spinning until it becomes available.
        pub fn lock(&self) -> SerialLockGuard<'_> {
            while self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                hint::spin_loop();
            }
            SerialLockGuard { lock: self }
        }
    }

    /// RAII guard that releases the lock on drop.
    pub struct SerialLockGuard<'a> {
        lock: &'a SerialLock,
    }

    impl Drop for SerialLockGuard<'_> {
        fn drop(&mut self) {
            self.lock.locked.store(false, Ordering::Release);
        }
    }

    /// Global serial test lock.
    pub static TEST_SERIAL_LOCK: SerialLock = SerialLock::new();
}

/// Re-export for convenient access from test modules.
#[cfg(test)]
pub use test_serial_lock::TEST_SERIAL_LOCK;
