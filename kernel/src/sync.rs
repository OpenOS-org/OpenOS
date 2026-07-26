//! Interrupt-safe mutual exclusion primitives.
//!
//! Standard spinlocks (`spin::Mutex`) do not disable interrupts. If a CPU
//! acquires a spinlock and then takes an interrupt whose handler tries to
//! acquire the same lock, the CPU deadlocks against itself.
//!
//! `IntMutex` solves this by disabling interrupts (CLI) before acquiring
//! the lock and restoring the previous interrupt state (STI) on unlock.
//! This guarantees that no interrupt handler can preempt the critical
//! section and attempt to re-acquire the lock.
//!
//! ## Usage
//!
//! Replace `spin::Mutex<T>` with `IntMutex<T>` for any global that is
//! accessed from both process context and interrupt handlers (e.g., the
//! scheduler, device state, timer queues).
//!
//! ## Deadlock scenario prevented
//!
//! ```text
//!   CPU 0                          CPU 0 (interrupted)
//!   ─────                          ───────────────────
//!   lock(mtx)                      ...
//!   // critical section            IRQ fires
//!                                  handler: lock(mtx)  ← DEADLOCK
//! ```
//!
//! With `IntMutex`, interrupts are disabled while the lock is held, so
//! the IRQ cannot fire until after `unlock()`.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Interrupt-safe mutual exclusion lock.
///
/// Combines a spinlock with interrupt flag management. When `lock()` is
/// called, interrupts are disabled (CLI) and the previous IF state is
/// saved. When the guard is dropped, the saved IF state is restored.
///
/// This prevents the self-deadlock scenario where a spinlock holder is
/// interrupted and the ISR tries to acquire the same lock.
pub struct IntMutex<T> {
    /// The protected data.
    data: UnsafeCell<T>,
    /// The spinlock: `true` means locked.
    lock: AtomicBool,
    /// Saved interrupt flag state from the most recent `lock()` call.
    /// `true` means interrupts were enabled before CLI.
    saved_if: AtomicBool,
}

// SAFETY: IntMutex provides mutual exclusion — only one thread/CPU can
// access the inner data at a time. The lock+CLI pattern ensures that
// even on the same CPU, no interrupt handler can access the data while
// the lock is held.
unsafe impl<T: Send> Sync for IntMutex<T> {}
unsafe impl<T: Send> Send for IntMutex<T> {}

impl<T> IntMutex<T> {
    /// Create a new interrupt-safe mutex protecting `data`.
    ///
    /// The lock starts unlocked and interrupts are not affected until
    /// `lock()` is called.
    pub const fn new(data: T) -> Self {
        Self {
            data: UnsafeCell::new(data),
            lock: AtomicBool::new(false),
            saved_if: AtomicBool::new(false),
        }
    }

    /// Acquire the lock, disabling interrupts.
    ///
    /// Returns an `IntMutexGuard` that restores the interrupt state when
    /// dropped. If the lock is already held (by another CPU or by code
    /// that interrupted us), this spins until it becomes available.
    ///
    /// # Interrupt behavior
    ///
    /// 1. Reads and saves the current RFLAGS.IF state
    /// 2. Disables interrupts (CLI)
    /// 3. Spins until the lock is acquired
    ///
    /// The guard's `Drop` implementation restores the saved IF state.
    pub fn lock(&self) -> IntMutexGuard<'_, T> {
        // Save the current interrupt flag state before disabling.
        // SAFETY: Reading RFLAGS is a non-privileged operation on x86_64.
        // We need the IF bit to restore it correctly on unlock.
        let interrupts_enabled = interrupts_enabled();

        // Disable interrupts before attempting to acquire the spinlock.
        // This prevents the deadlock scenario where an ISR tries to
        // acquire the same lock while we hold it.
        disable_interrupts();

        // Spin until we acquire the lock. With interrupts disabled, no
        // ISR can preempt us here.
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }

        // Store the saved IF state so the guard can restore it on drop.
        self.saved_if.store(interrupts_enabled, Ordering::Relaxed);

        IntMutexGuard { mutex: self }
    }

    /// Unlock the mutex and restore the saved interrupt flag state.
    ///
    /// This is called automatically by `IntMutexGuard::drop`. Do not
    /// call it directly — use the guard instead.
    fn unlock(&self) {
        // Read the saved IF state before releasing the lock.
        let was_enabled = self.saved_if.load(Ordering::Relaxed);

        // Release the lock.
        self.lock.store(false, Ordering::Release);

        // Restore the interrupt flag to its state before `lock()`.
        // If interrupts were enabled before we locked, re-enable them.
        if was_enabled {
            enable_interrupts();
        }
    }
}

/// RAII guard for `IntMutex`.
///
/// Holds the lock for the lifetime of this struct. When dropped, the lock
/// is released and the interrupt flag is restored to its pre-lock state.
pub struct IntMutexGuard<'a, T> {
    mutex: &'a IntMutex<T>,
}

impl<T> Deref for IntMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: The guard guarantees exclusive access to the data.
        // The lock is held and interrupts are disabled (on this CPU).
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for IntMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The guard guarantees exclusive access to the data.
        // The lock is held and interrupts are disabled (on this CPU).
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for IntMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

// ============================================================================
// Interrupt flag helpers
// ============================================================================

/// Check whether interrupts are currently enabled.
///
/// Reads the Interrupt Flag (IF, bit 9) from RFLAGS.
///
/// When testing on the host, this always returns `true` (interrupts
/// are assumed to be enabled) since privileged instructions are not available.
fn interrupts_enabled() -> bool {
    #[cfg(test)]
    {
        true
    }
    #[cfg(not(test))]
    {
        // SAFETY: Reading RFLAGS is always safe from any privilege level.
        // PUSHFQ + POP is the standard way to read RFLAGS without RDMSR.
        let rflags: u64;
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {}",
                out(reg) rflags,
                options(nomem, preserves_flags),
            );
        }
        // RFLAGS bit 9 is the Interrupt Flag (IF).
        const IF_BIT: u64 = 1 << 9;
        rflags & IF_BIT != 0
    }
}

/// Disable interrupts on the current CPU (CLI).
///
/// When testing on the host, this is a no-op since privileged
/// instructions are not available.
///
/// # Safety
///
/// Disabling interrupts prevents the CPU from servicing hardware
/// interrupts. The caller must ensure interrupts are re-enabled before
/// any long-running or blocking operation, and before returning to
/// code that expects interrupts to be enabled.
fn disable_interrupts() {
    #[cfg(not(test))]
    {
        // SAFETY: CLI only affects the current CPU's interrupt delivery.
        // It does not affect other CPUs. We always restore IF via the
        // IntMutexGuard drop, so interrupts are never left disabled
        // permanently.
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
        }
    }
}

/// Enable interrupts on the current CPU (STI).
///
/// When testing on the host, this is a no-op since privileged
/// instructions are not available.
///
/// # Safety
///
/// Enabling interrupts may immediately deliver a pending interrupt.
/// The caller must be in a context where interrupt delivery is safe
/// (i.e., not holding any non-reentrant locks without CLI protection).
fn enable_interrupts() {
    #[cfg(not(test))]
    {
        // SAFETY: STI re-enables interrupt delivery on the current CPU.
        // We only call this when restoring the IF state that was saved
        // before CLI, so the interrupt state is always restored to a
        // known-good value.
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_int_mutex_new_is_unlocked() {
        let m = IntMutex::new(42u32);
        assert!(!m.lock.load(Ordering::Relaxed));
    }

    #[test]
    fn test_int_mutex_lock_and_deref() {
        let m = IntMutex::new(100u32);
        {
            let guard = m.lock();
            assert_eq!(*guard, 100);
        }
        // After drop, lock is released.
        assert!(!m.lock.load(Ordering::Relaxed));
    }

    #[test]
    fn test_int_mutex_mut_deref() {
        let m = IntMutex::new(0u32);
        {
            let mut guard = m.lock();
            *guard = 999;
        }
        let guard = m.lock();
        assert_eq!(*guard, 999);
    }

    #[test]
    fn test_int_mutex_guard_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<IntMutexGuard<'_, u32>>();
    }

    #[test]
    fn test_int_mutex_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<IntMutex<u32>>();
    }

    #[test]
    fn test_if_bit_position() {
        // IF is bit 9 of RFLAGS.
        let if_bit: u64 = 1 << 9;
        assert_eq!(if_bit, 0x200);
    }

    /// `IntMutex` can protect different types.
    #[test]
    fn test_int_mutex_with_different_types() {
        let m_bool = IntMutex::new(true);
        let m_u64 = IntMutex::new(0xFFFF_FFFF_FFFF_FFFFu64);
        let m_tuple = IntMutex::new((1u8, 2u16, 3u32));

        assert!(*m_bool.lock());
        assert_eq!(*m_u64.lock(), 0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(*m_tuple.lock(), (1, 2, 3));
    }

    /// Lock, modify, unlock, re-lock: value persists.
    #[test]
    fn test_int_mutex_persistence_across_locks() {
        let m = IntMutex::new(String::new());
        m.lock().push_str("hello");
        m.lock().push_str(" world");
        assert_eq!(&*m.lock(), "hello world");
    }

    /// `IntMutex::new` is const (can be used in static context).
    #[test]
    fn test_int_mutex_const_new() {
        static M: IntMutex<u32> = IntMutex::new(42);
        assert_eq!(*M.lock(), 42);
    }

    /// `saved_if` starts as false.
    #[test]
    fn test_saved_if_starts_false() {
        let m = IntMutex::new(0u32);
        assert!(!m.saved_if.load(Ordering::Relaxed));
    }

    /// Multiple sequential locks and unlocks work correctly.
    #[test]
    fn test_int_mutex_sequential_locks() {
        let m = IntMutex::new(0u32);
        for i in 0..100 {
            *m.lock() = i;
        }
        assert_eq!(*m.lock(), 99);
    }

    /// `IntMutex` with a large payload.
    #[test]
    fn test_int_mutex_large_payload() {
        let data = [0xABu8; 4096];
        let m = IntMutex::new(data);
        let guard = m.lock();
        assert_eq!(guard[0], 0xAB);
        assert_eq!(guard[4095], 0xAB);
    }

    /// Guard implements `DerefMut` (can modify through shared reference).
    #[test]
    fn test_int_mutex_guard_deref_mut() {
        let m = IntMutex::new(Vec::new());
        {
            let mut guard = m.lock();
            guard.push(1);
            guard.push(2);
            guard.push(3);
        }
        assert_eq!(m.lock().len(), 3);
    }

    /// Nested lock on different `IntMutex` instances works.
    #[test]
    fn test_int_mutex_nested_different_instances() {
        let m1 = IntMutex::new(10u32);
        let m2 = IntMutex::new(20u32);
        let g1 = m1.lock();
        let g2 = m2.lock();
        assert_eq!(*g1 + *g2, 30);
    }
}
