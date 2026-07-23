//! Task management and scheduling.
//!
//! Provides task creation, state tracking, and a round-robin scheduler.
//! Context switching (register save/restore, page table switching) is not
//! yet implemented — the kernel runs a single "idle" task in a `hlt` loop.

use crate::println;

pub mod scheduler;
pub mod task;

/// Initialize the task subsystem and create the idle task.
pub fn init() {
    println!("[...] Initializing task scheduler");
    scheduler::init();
    println!("[OK] Task scheduler initialized");
}
