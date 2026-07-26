//! Task management and scheduling.
//!
//! Provides task creation, state tracking, a round-robin scheduler, and
//! user-mode process launching.

use crate::println;

pub mod scheduler;
pub mod task;
pub mod user;

/// Initialize the task subsystem and create the idle task.
pub fn init() {
    println!("[...] Initializing task scheduler");
    scheduler::init();
    println!("[OK] Task scheduler initialized");
}
